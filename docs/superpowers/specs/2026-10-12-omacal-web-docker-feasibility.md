# Putting OmaCal in a Docker container as a web app

What the **minimum** is, to run OmaCal as a self-hosted web app reachable from a
browser while away from the machine that runs the desktop app — recorded while
the ground is fresh (2026-10-12, against `v5.5.0`). **Nothing here is a
commitment to build it.** It is the inventory, so the next person to ask does not
have to re-derive it, and so a decision to start begins from facts.

Companions: the Tauri/IPC facts are researched in
`.scratch/web-app-docker/research/01-tauri-frontend-in-a-browser.md`, the Google
facts in `…/02-google-oauth-for-a-hosted-app.md`, and the image facts in
`…/03-minimal-rust-sqlite-image.md`. The shape of this document mirrors
[`2026-08-31-omacal-windows-port-feasibility.md`](2026-08-31-omacal-windows-port-feasibility.md).

## The short version

The Docker part is the **easy** part — a Rust binary plus static files on a
volume, and the whole dependency graph already links statically (bundled SQLite,
rustls/`ring`, no OpenSSL). The hard part is that OmaCal is not a client of a
server; it **is** the server, living in the same process as its UI and reaching
for the operating system to hold its secrets, draw its tray, fire its
notifications and open its browser.

So the minimum is not a `Dockerfile`. It is **three seams**:

1. **The UI's transport.** The Svelte build is already a plain static SPA; only
   the `@tauri-apps/api` layer assumes a webview. One build-time shim that maps
   `invoke` → `fetch` and `listen` → a WebSocket is enough to run the existing UI
   in a browser. *(Research 01: confirmed, no Tauri web target exists.)*
2. **The backend's edge.** The business logic must be reachable without a Tauri
   `AppHandle`. Most of it already is — the `*_body`/`*_impl` split in
   `src-tauri/src` exists precisely so tests can call it — but `emit(...)`,
   `app.path()` and the background loops still leak Tauri into the bodies.
3. **The OS boundaries the container lacks.** The refresh tokens live in the
   **Secret Service/keychain** (`keyring`), and there is **no login of its own**
   — the OS user *was* the authentication. Both must be replaced with
   server-side equivalents before the app can face a network.

And one external fact that does not bend: **Google sign-in moves from the
installed-app flow to a "Web application" OAuth client**, a new client id/secret,
a registered HTTPS callback, the secret kept server-side, and **every existing
user re-consenting**. *(Research 02.)*

## What already works, unchanged

**The whole domain core has no Tauri in it at all.** `omacal-core` (recurrence,
layout, reminders, search), `omacal-store` (SQLite, migrations, the accent fold),
`omacal-sync` (CalDAV + WebCal + the Google converters) and `omacal-google` /
`omacal-caldav` (the HTTP clients and the OAuth plumbing) are plain library
crates. They compile and test on a headless box today.

**The whole UI.** `ui/dist` is an ordinary static SPA — `vite build` output, no
router, no SSR, no Tauri-only markup. It builds and runs in a browser; the only
thing that breaks is the API layer (the shim, below). The UI uses exactly two
Tauri surfaces: `invoke` (17 files, ~63 commands) and `listen` (3 files, 9
server-pushed events). It never calls `emit`, never imports
`@tauri-apps/plugin-opener`, and never touches `window`/`webview`/`path`.
*(Research 01 §4.)*

**The CLI's read path.** `omacal agenda|events list|search|…` already opens
`omacal.db` directly, with no app running (`src-tauri/src/cli.rs:650`). Pointed
at the container's volume (`XDG_DATA_HOME=/data`), reads keep working as-is. Only
its *write* half (`ipc.rs`, a Unix socket to the running app) needs a new
transport, and that can be deferred.

**CalDAV, iCloud and WebCal.** App-specific passwords and subscription feeds are
ordinary outbound HTTP; nothing about them assumes a desktop. The connect
commands store the password in the keyring — the same boundary as Google, with
the same fix.

**Static linking and TLS.** `sqlx`'s `sqlite` feature compiles `sqlite3.c` into
the binary; `reqwest` uses `rustls` with the `ring` provider and the compiled-in
`webpki-roots` are already enabled. The runtime needs **no** system SQLite, **no**
OpenSSL and **no** `ca-certificates` files. *(Research 03 §1–§3.)*

## The three seams, in detail

### Seam 1 — the UI's transport (small, certain)

Tauri v2 has **no web/server target**; the docs call it "a static web host" and
state it "does not natively support server based alternatives"
([Frontend Configuration](https://v2.tauri.app/start/frontend/)). The frontend
build is therefore already portable — only `invoke` is not: it is literally
`window.__TAURI_INTERNALS__.invoke(...)`, and with no webview that object is
absent, so the call rejects with a `TypeError`.

The clean fix — one file, imported as the first line of `ui/src/main.ts` —
installs `window.__TAURI_INTERNALS__` before Svelte boots, closely modelled on
Tauri's own `@tauri-apps/api/mocks` `mockIPC()` (whose docstring explicitly
sanctions running the frontend outside the webview):

- `invoke(cmd, args)` → `POST /api/<cmd>` with the args as JSON; resolve the
  response body on 2xx, **throw the response's error string** on non-2xx (Tauri
  rejects with the Rust `Err`'s `Display`, and existing `.catch` paths depend on
  it).
- `plugin:event|listen` / `|unlisten` are intercepted locally and the event name
  registered; a single **WebSocket** from the server dispatches the nine events
  through `__TAURI_INTERNALS__.runCallback`.
- `plugin:opener|*` is a no-op today (unused); map it to `window.open` if it ever
  appears.

Pitfall to avoid: a Vite alias of `@tauri-apps/api/core` **alone does not work**,
because `@tauri-apps/api/event` imports `./core.js` *relatively* and slips past
specifier-based aliasing. Installing the internals object is the whole strategy.
*(Research 01 §3.)*

### Seam 2 — the backend's edge (the real work)

The server must answer one route per command without a Tauri `AppHandle`. The
existing code is closer to this than it looks: every write already has a
`*_body` entry point (`events::create_event_body`, `tasks::complete_body`, …)
that the CLI's Unix socket in `ipc.rs` calls without a webview, and reads are
`*_impl` functions (`get_days_impl`, `search`, `status::read_status`). What still
couples a body to Tauri is three things:

- **Refresh emissions.** Bodies call `app.emit("sync-finished", …)` (and the
  other eight events) to tell the open UI to reload. `sync_loop.rs`, `events.rs`,
  `tasks.rs`, `calendars.rs`, `settings.rs`, `invites.rs`, `weather.rs` and
  `ipc.rs` all reach for this. The codebase already has the pattern to fix it:
  **`notify::Notifier` is a one-method trait** with two implementations, wired in
  one place — an event sink is the same shape.
- **Path lookups.** `app.path().app_data_dir()` in `settings.rs` and `lib.rs`;
  replace with the existing `cli::app_data_dir()` (already written to work
  without an app).
- **The loops.** `sync_loop::spawn(app)` / `notify_loop::spawn(app, notifier)`
  take an `AppHandle` only to emit; give them the event sink instead.

Two ways to place the result:

| | **In-crate server** | **Extracted service crate** *(recommended)* |
|---|---|---|
| Where | a `serve()` in `src-tauri`, or a `src/bin/` bin | new `omacal-app` crate: the Tauri-free bodies; `src-tauri` and `omacal-server` both depend on it |
| Code change | small — `pub`-ify bodies, add routes | medium — move the service layer out, add routes |
| `tauri` still compiled? | **yes** → WebKitGTK/GTK in the builder **and** runtime | **no** → distroless, ~22 MB |
| Image | several hundred MB (GTK/WebKit stack) | ~22 MB + binary |

The in-crate server is the smaller diff but keeps the desktop stack in the image,
for no benefit once the tray and window are gone. Extracting is the honest
minimum for a result worth keeping.

### Seam 3 — the OS boundaries the container lacks

- **Secrets.** Google refresh tokens and CalDAV passwords live in the OS keyring
  (`KEYRING_SERVICE = "omacal"`; `src-tauri/src/lib.rs:432,504`,
  `caldav_account.rs:31`). A container has no Secret Service. The web build must
  swap the `keyring` backend for **encrypted material on the volume** — a `0600`
  file or a table in `omacal.db`, sealed with an AEAD keyed by a master key from
  the environment / a Docker secret. This is a **hard prerequisite**, not a
  mount detail. *(Research 03, "OmaCal-specific blocker".)*
- **Login.** The desktop app has no session: the OS user is the auth. A web app
  needs one before it faces a network. The least work that is still safe is
  **not to expose the container at all** — put it on a private network (below) —
  and, if it must be public, a single-user password + signed session cookie in
  front of every `/api/*` route.

## Google sign-in from a server

The desktop flow (`crates/omacal-google/src/auth.rs`) binds an ephemeral
`http://127.0.0.1:<port>` listener and reads the code off it. From a container
that redirect goes to the **browser's** host, not the server, and a **Desktop**
OAuth client has no field in which to register a public HTTPS callback at all.
*(Research 02 §2–§3.)*

The changes, all confirmed against Google's docs:

- **New client, type "Web application"**, with a registered `https://…/callback`
  redirect (exact match: scheme, case, trailing slash). `access_type=offline` and
  `prompt=consent select_account` carry over **unchanged** — they are still what
  yields and refreshes the refresh token.
- **The client secret becomes a real secret.** For installed apps Google says the
  secret "is obviously not treated as a secret"; a Web application is a
  **confidential client** expected to keep it server-side. It moves from a value
  baked into the binary (`OMACAL_CLIENT_SECRET`) to an env var / Docker secret
  that **never reaches the browser**. This is strictly *more* protectable than
  today.
- **PKCE is optional.** Google documents PKCE only for installed apps; the
  web-server guide is silent on it. Keeping S256 is harmless if the live endpoint
  accepts it, but it is no longer load-bearing once a confidential secret exists.
- **Re-consent for everyone.** Refresh tokens are bound to the minting client, so
  the existing desktop grants do not carry over; the web client needs a fresh
  grant per account. The existing error classifier already names this case
  (`unauthorized_client`/`invalid_client`, `auth.rs:131`) and treats it as
  "re-connect", so the plumbing to prompt for it exists.
- **Consent-screen state is unchanged and still the long pole.** `calendar` is a
  *sensitive* scope (free review, not the paid assessment); an **unverified
  production app has a hard, lifetime 100-new-user cap**; and an app left in
  **Testing** expires refresh tokens **every 7 days**. For a personal instance
  (one account, a handful of users) unverified is fine forever; for open
  distribution, publish and verify. `docs/distribution.md` §2 is accurate here —
  only its "100-user cap applies to new grants" phrasing should be tightened to
  "100 new users, lifetime, non-resettable".
- **Store and reuse** one refresh token per account: `prompt=consent` mints a new
  one each time, and Google caps at **100 refresh tokens per account per client**
  (oldest silently dies), plus a **6-month non-use expiry**.

The Rust pieces that survive almost intact: `authorize_url`, `exchange_code`,
`refresh`, `revoke`, `needs_reauth`. What changes is the client type, the
redirect (an HTTP route instead of `bind_loopback`/`wait_for_redirect`), and the
secret's source.

## What cannot come along

These are desktop-only and should be dropped or stubbed rather than ported. None
of them is on the critical path for a calendar in a browser:

- **Tray and macOS menu bar** — `tray.rs`, `tray_date.rs`, `menubar.rs`.
- **Native notifications** — `notify.rs` (`DbusNotifier`/`MacNotifier`),
  `notify_mac.rs`. Keep the **scheduler** (`notify_loop.rs` — the reminder rules
  live in `omacal-core::remind`), replace only the transport.
- **Self-update and restart** — `update.rs`, `restart.rs`. The container image is
  the update channel; the app's own "Update" banner and `install_update` do not
  apply.
- **Omarchy theming and bar widget** — `theme.rs`, `theme_watch.rs`,
  `system_theme.rs`, `omarchy_plugin.rs`. Stub to a default palette; the UI's own
  settings still colour things.
- **Suspend watcher / tz watcher / ICU sidecar** — `resume.rs`, `tz_watch.rs`,
  `icu_tz.rs`. The server never suspends; ship `tzdata` in the image instead.
- **GTK/WebKit environment hacks** — `nvidia.rs`, `apply_gtk_*`: gone with the
  webview.
- **File dialogs and OS opener** — `tauri_plugin_dialog` in `import.rs` /
  `export.rs`, and `browser.rs`. Export becomes a download; import becomes an
  upload; meeting links become ordinary anchors (the browser is already there).
- **Single-instance / autostart plugins** and the argv control channel
  (`tray::instance_action`) — container lifecycle replaces them.
- **The pinch and edge-cursor shims** — `pinch.ts`/`pinch.rs`, `edge_cursor.rs`;
  the browser does both.

## The Docker image itself

The clean path is unglamorous because the stack is already well-behaved:

- **Builder:** `node:22-bookworm-slim` → `npm ci && npm run build` for `ui/dist`;
  `rust:1-trixie` (glibc, matching the runtime's Debian) with `cargo-chef` and
  BuildKit cache mounts for the server binary. No `DATABASE_URL`/`.sqlx` needed —
  the store uses only `sqlx::migrate!` and runtime queries.
- **Runtime:** **`gcr.io/distroless/cc-debian13:nonroot`** (~22 MB, uid 65532,
  no shell). `webpki-roots` being compiled in means no cert files; include
  `tzdata`. A musl-static build on `distroless/static` is viable and slightly
  smaller but buys cross-compilation for little gain on a personal instance.
- **Volume:** mount at the app data dir and set `XDG_DATA_HOME=/data`; the DB is
  `com.omacal.app/omacal.db` (+ `-wal`, `-shm`, `0600`). WAL needs a **local**
  filesystem — a Docker named volume qualifies; a network bind does not.
- **Ops:** bind `0.0.0.0:8080`; run non-root with `/data` owned by the runtime
  uid; `HEALTHCHECK` in **exec form** against a tiny `/healthz` route (there is no
  shell or curl in distroless); serve `ui/dist` with `tower-http`'s `ServeDir`.
  *(Research 03 §4–§5.)*

A concrete multi-stage skeleton, and the compose volume stanza, are in Research 03.

## Reaching it from away from home

Two postures; the minimum picks the first:

- **Private overlay (recommended, least work).** Run the container on a home box
  or small VPS and reach it over **Tailscale / WireGuard**. The app then needs no
  public TLS termination and no login of its own — the network *is* the
  authentication. This is the smallest correct thing for "just me, from my
  phone", and it sidesteps the entire auth seam for phase 1.
- **Public HTTPS.** A reverse proxy (Caddy/Traefik) terminating TLS in front, a
  login on `/api/*`, and the consent-screen/verification consequences above. More
  surface, more work; needed only to share the instance.

Either way the container binds `0.0.0.0:8080` and the proxy/overlay sits in front;
nothing in the app should ever listen on the open internet directly.

## Minimum viable scope, in two phases

**Phase 1 — one browser, one account, over a private network. ~1–2 weeks.**
Extract the service layer; axum server with the ~63 routes; the UI shim
(`invoke`→`fetch`, `listen`→WS); file-based encrypted secrets on the volume; new
Web-application OAuth client + callback route; `ui/dist` + distroless image +
compose; `sync-finished` over the WebSocket. Defer the CLI's write half, all
desktop-only subsystems, Web Push, multi-user, and public auth (Tailscale
covers it). The result is the real OmaCal, on the web, from your phone.

**Phase 2 — only if it earns it. ~1–2 weeks.**
Web login + public TLS; server-side reminders/cancel-notice push with the
in-page Notification API (the invite tray UI already exists); HTTP upload/download
for `.ics` import/export; reconnect the CLI's write path over localhost HTTP;
tighten images and add CI.

**A zero-code baseline, for completeness.** Before any of this, the literal
minimum to "have it in a Docker container" today is to containerise the *desktop*
app: the AppImage under `Xvfb` + `x11vnc` + `noVNC`, with `gnome-keyring` for the
Secret Service. It works untouched — even the OAuth loopback lands in the
container's own browser — but it streams a desktop rather than being a web app:
heavy, awkward on a phone, and it keeps every desktop subsystem. Good for a
same-day proof; not the thing to keep.

## The one cost that is not on any of these lists

**This is a second front end to maintain.** Today there is one UI and one
backend, inseparably joined. After this there are two hosts for the same service
layer, two auth models, two notification transports, and a shim whose fidelity to
Tauri's `invoke`/event wire behaviour is a standing thing to keep true. The
codebase is unusually well-placed for it — the service layer is mostly already
separable and the UI already speaks a command/event vocabulary — but the
"minimum" is still a permanent fork in the road, not a one-off script.

## Open decisions (not yet resolved)

- **Single-user vs multi-user** store: one DB per instance, or a tenant column /
  per-user DB. Phase 1 assumes one account, one instance.
- **Session auth shape**, if it ever leaves the private network: one shared
  password, per-user passwords, or an external identity provider.
- **The CLI's fate** on a server: reads-only, or writes over localhost HTTP.
- **Reminder delivery**: WebSocket + in-page notification (phase 2 proposal) vs
  Web Push (works with the tab closed, but needs a service worker and VAPID keys).

## Sources

- Tauri/IPC: `.scratch/web-app-docker/research/01-tauri-frontend-in-a-browser.md`
  — Tauri v2 [Frontend Configuration](https://v2.tauri.app/start/frontend/),
  [Mock Tauri APIs](https://v2.tauri.app/develop/tests/mocking/), upstream issue
  [#13595](https://github.com/tauri-apps/tauri/issues/13595).
- Google: `.scratch/web-app-docker/research/02-google-oauth-for-a-hosted-app.md`
  — [OAuth 2.0 overview](https://developers.google.com/identity/protocols/oauth2),
  [web-server flow](https://developers.google.com/identity/protocols/oauth2/web-server),
  [installed-app flow](https://developers.google.com/identity/protocols/oauth2/native-app),
  [Manage OAuth Clients](https://support.google.com/cloud/answer/15549257),
  [sensitive-scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification).
- Image: `.scratch/web-app-docker/research/03-minimal-rust-sqlite-image.md` —
  distroless base README, `sqlx`/`libsqlite3-sys` docs, `reqwest` feature flags,
  Docker volumes and `HEALTHCHECK` references.
- Internal: `docs/distribution.md` §1–§2; `crates/omacal-google/src/auth.rs`;
  `src-tauri/src/lib.rs`, `ipc.rs`, `cli.rs`, `sync_loop.rs`, `notify_loop.rs`.
