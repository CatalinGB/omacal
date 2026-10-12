# OmaCal web-app MVP: the smallest thing that shows your calendar in a browser

Scoping for a **view-only** OmaCal served from a Docker container and reached
from a browser while away from the laptop — recorded 2026-10-12, branch
`research/web-app-mvp` off `main` (`f078d9d`). It follows the feasibility report
([`2026-10-12-omacal-web-docker-feasibility.md`](2026-10-12-omacal-web-docker-feasibility.md)),
which found that the hard parts are the UI transport, the Tauri edge, and the OS
boundaries. **This narrows all three to the minimum by giving up writes.**

Evidence lives under `.scratch/web-app-mvp/` (map, tickets, research); the
Google token findings in
`.scratch/web-app-mvp/research/02-reuse-desktop-refresh-token.md`.

## The short version

A read-only server is a **thin skin over code that already exists**. Every read
command in `src-tauri` is `pool`-only — no `AppHandle` — and the layer that turns
stored events into week/month/year payloads (`commands.rs`) is a set of pure
functions. Sync is already written to take its **token source as a parameter**,
because tests needed it that way. So the MVP does not need the service-layer
extraction the full report budgeted: it needs the pure layer moved somewhere
`src-tauri` and the new server can both reach.

The only genuinely new pieces are four: a **server crate** with routes and a
WebSocket, a **file-backed token store** to replace the keyring, the **UI fetch
shim**, and the **Dockerfile**. Writes, the web OAuth consent flow, notifications
and every desktop subsystem are **out of the MVP**.

## What the MVP is

One user, one Google account, **read-only**: the Week, Day, Month and Year views,
the calendar list, the Tasks pane and search — the real Svelte UI, served from
`ui/dist`, talking to the real sync engine's data. No creating, moving, deleting
or answering; no reminders; no multi-user. Reached over a private network
(Tailscale/WireGuard) so it needs no login of its own.

## Why it is small: what is already portable

Confirmed by reading the tree (ticket `01-read-only-server-surface`):

- **Reads never touch Tauri.** `get_week`/`get_day`/`get_range` →
  `get_days_impl(&SqlitePool, …)` (`lib.rs:312`); `get_month` (`:344`),
  `get_year` (`:373`), `get_big_year` (`:396`) read
  `settings::read_settings` + `omacal_store::events_in_window` and call the
  assemblers. `get_settings`, `get_calendars`, `list_tasks`, `task_lists`,
  `search` take only `State<AppState>` and use `state.pool`.
- **The assemblers are pure.** `commands.rs` has exactly one `tauri::` reference
  — the `open_conference` **command wrapper** (`:924`), not an assembler.
  `assemble_week`/`_month`/`_year`/`_big_year`/`_days` are functions over
  `&[StoredEvent]`.
- **Sync is injectable.** `sync_accounts<F, Fut>(pool, accounts, api_base,
  window, token_for)` (`lib.rs:872`) is pool-only and takes the token source as
  a closure — its doc says so explicitly ("reachable from a test"). Only
  `sync_loop.rs` reaches for `AppHandle`, and only to `emit`.
- **The domain crates already have no Tauri.** `omacal-core`, `-store`, `-sync`,
  `-google`, `-caldav`.

**The one mechanical step this implies:** move `commands.rs` and the pool-only
read/sync helpers (`get_days_impl`, `get_month`/`get_year`/`get_big_year_impl`,
`display_tz`, `now_ms`, `sync_accounts`) into a new Tauri-free crate — call it
`omacal-view` — that `src-tauri` and the server both depend on. The
`#[tauri::command]` wrappers stay in `src-tauri` and call into it. That is the
whole extraction, and it is a move, not a rewrite.

## The MVP architecture

```
browser ──HTTP──▶ omacal-server (axum)  ──▶ SqlitePool ──▶ volume (/data)
        ◀──WS────                    │
                                     └─▶ sync ticker ──▶ Google/CalDAV
```

- **`omacal-server`** — a new workspace crate (axum + tokio + tower-http), built
  on `omacal-view`, `omacal-sync`, `omacal-store`. It owns a `SqlitePool`, a
  small `AppState` (`pool`, `tokens`, `reauth`, an event sink), and nothing else.
  Routes are one per read command, `/api/<cmd>`; `get_status`, `get_palette` and
  `appearance_previews` are stubbed to defaults (they read the desktop's title
  bar, package version and Omarchy theme).
- **Static UI** — `tower-http`'s `ServeDir` serves `ui/dist` at `/`; the shim
  talks to `/api/*` and one WebSocket.
- **Sync ticker** — a tokio task on the same interval logic as `sync_loop`
  (`sync_loop::interval_ms`, floor 60 s). It calls `sync_accounts` with a token
  source backed by the server store, and broadcasts `sync-finished` over the
  WebSocket instead of `emit`ing to a webview. This is what keeps the DB fresh;
  a read-only *UI* is not a read-only *server*.
- **Token store** — a `0600` file on the volume, one refresh token per account
  email. MVP reads it directly (encryption-at-rest is phase 2; the volume is
  private and single-user). It replaces `load_refresh_token`/`store_refresh_token`
  (`lib.rs:504-511`), which are the only keyring callers on the sync path.
- **WebSocket** — one channel; the client `listen`s and the shim dispatches.
  Read-only needs only `sync-finished` (and `sync-failed`); the other seven
  events are desktop-driven and never fire.

## Getting a token in (the one real shortcut)

The full web path wants a new "Web application" OAuth client and browser
consent — expensive. The MVP instead **imports the token the desktop already
holds**, under the **same** client pair. Research verdict:
**caveated-viable**.

- A Google refresh token is bound to the **client**, not the device — no IP or
  platform binding — so a headless server can refresh it. The same
  `client_id`/`client_secret` is required; the secret is optional in the refresh
  grant.
- On Linux the token is in the Secret Service under `service=omacal`,
  `username=<email>`. One command copies it out:

      secret-tool lookup service omacal username you@example.com

  piped into the container's store once. (The desktop and the server refreshing
  the same token concurrently is safe — Google does not rotate on refresh.)
- **Caveats to design around.** If the OAuth consent screen is in **Testing**,
  the refresh token dies every **7 days** — so either publish the consent screen
  (production, unverified is fine for one account) or re-import weekly. Google's
  own policy says not to use user credentials for a server deployment; that is a
  **compliance** concern, not a technical one, and is acceptable for a personal
  single-user instance but is exactly what a phase-2 web OAuth client fixes.

## The UI shim (read-only)

One file, imported first in `ui/src/main.ts`, installs
`window.__TAURI_INTERNALS__` (modelled on Tauri's own `mocks`):

- `invoke(cmd, args)` → `POST /api/<cmd>`; throw the server error string on
  non-2xx, resolve the JSON body on 2xx.
- `plugin:event|listen`/`|unlisten` intercepted locally; the WebSocket dispatches
  into `__TAURI_INTERNALS__.runCallback`.
- `ui/dist` needs no change: it is a plain SPA served at `/` (no router, no SPA
  fallback).

For a view-only MVP the write buttons are the honest problem: they will hit
routes that do not exist. The cheap answer is a **read-only banner** and
disabling the write affordances in the shim; a fuller answer (a later ticket) is
to hide them in the UI. This is the main thing the prototype ticket
(`03-ui-shim-surface`) should settle.

## What is explicitly out

Writes (create/update/delete/respond) and their guards; the interactive web OAuth
client and its callback route; reminders, cancel-notice pushes and the invite
tray's live answers; multi-user; and every desktop subsystem the feasibility
report already rules out (tray, menu bar, native notifications, self-update,
GTK/Omarchy theming, file dialogs, single-instance, autostart, suspend/tz
watchers). The CLI's write half stays undone; reads work against the volume.

## Packaging and access

Straight from research `03-minimal-rust-sqlite-image`: glibc build on
`rust:1-trixie`, run on `gcr.io/distroless/cc-debian13:nonroot` (~22 MB); bundled
SQLite + `webpki-roots` mean no SQLite/OpenSSL/cert files; `tzdata` is the only
extra. Volume at `XDG_DATA_HOME=/data`, non-root uid 65532, exec-form
`HEALTHCHECK` against `/healthz`, bind `0.0.0.0:8080`. The server must **not**
depend on `src-tauri`, or Tauri/WebKitGTK get pulled in. Reach it over
**Tailscale/WireGuard** — the network is the auth, so phase 1 needs no login.

## Work breakdown

| # | Task | Size |
|---|---|---|
| 1 | `omacal-view` crate: move `commands.rs` + the pool-only read/sync helpers; rewire `src-tauri` to it; keep tests green | M |
| 2 | `omacal-server` crate: axum, `AppState`, `/api/*` read routes, `/healthz`, static `ui/dist`, stubbed `get_status`/`get_palette` | M |
| 3 | File-backed token store + token-import path (`secret-tool` → volume) | S |
| 4 | Sync ticker calling `sync_accounts` with the store's token source; WS broadcast of `sync-finished`/`sync-failed` | S–M |
| 5 | UI `main.ts` shim (invoke→fetch, listen→WS) + read-only banner/affordance gating | M |
| 6 | Dockerfile + compose (multi-stage, distroless, volume, healthcheck) | S |
| 7 | End-to-end: seed the token, boot, verify the four views + tasks + search from a phone over Tailscale | M |

Rough total: **~1 week** to a working view-only instance, assuming the token
bootstrap is acceptable.

## Risks and open decisions

- **The 7-day token cliff** if the consent screen is left in Testing. Mitigate by
  publishing the consent screen (one account) before relying on it.
- **Read-only UI affordances** — a view-only app whose Save button errors reads as
  broken. The shim must disable them; hiding them is a UI change to schedule.
- **`display_tz`** uses the process zone; the container must set `TZ` to the
  user's zone or every day boundary is off. (A setting, not a rewrite.)
- **Open (ticket `04-mvp-scope-decision`):** confirm view-only is enough for the
  first cut; confirm Tailscale-only access; confirm single account; confirm the
  token-import bootstrap is acceptable versus doing the web OAuth client now.

## Sources

- `.scratch/web-app-mvp/issues/01-read-only-server-surface.md` (code analysis)
- `.scratch/web-app-mvp/research/02-reuse-desktop-refresh-token.md` (Google
  refresh-token reuse; `secret-tool` extraction; caveats)
- `.scratch/web-app-docker/research/01-tauri-frontend-in-a-browser.md` (the shim)
- `.scratch/web-app-docker/research/03-minimal-rust-sqlite-image.md` (the image)
- `docs/superpowers/specs/2026-10-12-omacal-web-docker-feasibility.md`
