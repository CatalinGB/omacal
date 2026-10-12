# OmaCal web-app MVP: your calendar in a browser, from the Radicale box

The MVP, scoped with you on 2026-10-12 (branch `research/web-app-mvp` off `main`
`f078d9d`). It follows the feasibility report
([`2026-10-12-omacal-web-docker-feasibility.md`](2026-10-12-omacal-web-docker-feasibility.md)),
but a grilling — and a look at what your desktop is *actually* connected to —
reshaped it: **your calendar is CalDAV on your own Radicale server, there is no
Google account, and the whole OAuth half of the plan falls away.**

Evidence lives under `.scratch/web-app-mvp/` (map, tickets, research).

## Settled decisions

| # | Decision |
|---|---|
| 1 | **Full read-write** — create/update/delete/respond events, task writes, calendar select/colour/label/sync, app settings. |
| 2 | Container runs on the **always-on box at 192.168.1.200** (the Radicale host), reached over **Tailscale**. No app login; the overlay network is the auth. |
| 3 | **One account, one user.** The CalDAV account `kalio` at `http://192.168.1.200:5232/kalio`, 9 collections incl. a VTODO list. |
| 4 | **Credential imported once** from the desktop's keyring. |
| 5 | **Core writes only.** Account management, `.ics` import/export and reminders/notifications are **out**. |
| 6 | **One shot**, not staged. |
| 7 | **CalDAV only — no Google, no OAuth, no consent screen.** |

## The short version

Two facts make this MVP much smaller than the feasibility report budgeted.

First, **the read path is already free of Tauri**: every read command is
`pool`-only and the layer that turns stored events into week/month/year payloads
is a set of pure functions. Second, and more important, **your provider is
CalDAV**: there is no OAuth to build, no Web-application client to register, no
consent screen to publish, no re-consent, and no token that expires every seven
days. The server authenticates to Radicale with a username and password it copies
from the desktop once, and everything else — sync, etag-guarded writes, the
guards, the error text — **already exists and is already server-callable** (the
CLI drives exactly those bodies over its socket today).

So the work is: move the Tauri-free service layer into a shared crate, put an
HTTP + WebSocket skin on it, shim the UI's `invoke`, and package it. No new
business logic.

## Your setup, as found

- **Account:** `caldav` provider, `server_url = http://192.168.1.200:5232/kalio`,
  username `kalio`. One account, nine calendars/collections (including
  `vtodo-tasks`). No Google row.
- **Credential:** the CalDAV password, in the OS keyring under
  `service=omacal, username=caldav:kalio`:
  `secret-tool lookup service omacal username caldav:kalio`.
- **Host:** this laptop is `herpo` (192.168.1.183) — the machine you take away,
  so not the host. The host is **192.168.1.200**, Docker-ready Linux x86_64.
- **Timezone:** the container must run `TZ=Europe/Bucharest`, or every day
  boundary is off (desktop `display_tz` reads the process zone).

## What already works, unchanged

- **Reads.** `get_week`/`get_day`/`get_range` → `get_days_impl(&SqlitePool, …)`
  (`lib.rs:312`); `get_month` (`:344`), `get_year` (`:373`), `get_big_year`
  (`:396`) read settings + `events_in_window` and call the pure assemblers.
  `get_settings`, `get_calendars`, `list_tasks`, `task_lists`, `search` use only
  `state.pool`. `commands.rs` has exactly one `tauri::` reference — the
  `open_conference` command wrapper, not an assembler.
- **Writes.** The `*_body` functions in `events.rs`, `tasks.rs`, `calendars.rs`,
  `settings.rs` and `caldav_write.rs` are the same ones the CLI reaches through
  `ipc.rs` (`dispatch`, `ipc.rs:164`). That is a **worked template** for the
  server's write routes — parse, guard, provider write, sanitized error.
- **Sync.** `sync_accounts` (`lib.rs:872`) is pool-only and takes its token
  source as a parameter. `sync_loop.rs` uses `AppHandle` only to `emit`.
- **Domain crates.** `omacal-core`, `-store`, `-sync`, `-caldav`, `-google` have
  no Tauri in them at all.

## The architecture

```
phone ──Tailscale──▶ omacal-server (axum, :8080) ──▶ SqlitePool (volume /data)
                     on 192.168.1.200   │                    │
                          ▲── WebSocket │                    ▼
                          └── sync ─────┴────────▶ Radicale http://192.168.1.200:5232
```

- **`omacal-view`** (new shared crate) — the Tauri-free service layer: the read
  assemblers and helpers, the write bodies, and `sync_accounts`. `src-tauri`'s
  `#[tauri::command]` wrappers call into it; the server does too. A **move**, not
  a rewrite: `emit(...)` becomes a small event-sink trait (the `notify::Notifier`
  pattern already in the tree), and `keyring` reads become a credential store.
- **`omacal-server`** (new crate, axum + tokio + tower-http) — one route per
  command at `/api/<cmd>`, a `SqlitePool`, a small `AppState` (pool, credential
  store, event sink), `/healthz`, and `ServeDir` for `ui/dist`. No `tauri`, no
  webview, no `src-tauri` dependency.
- **Sync ticker** — a tokio task on `sync_loop::interval_ms` (floor 60 s) calling
  `sync_accounts` with the CalDAV credential, broadcasting `sync-finished` over
  the WebSocket instead of `emit`ing to a webview.
- **WebSocket** — one channel; the client `listen`s. Only `sync-finished` (and
  `sync-failed`) ever fires; the other seven events are desktop-driven.
- **Static UI** — `ui/dist` served at `/`, unchanged; the shim talks to `/api/*`.

## Getting the credential in

One command, once:

    secret-tool lookup service omacal username caldav:kalio

piped into the container's credential file on the volume (`0600`, one password
keyed by account). The stored `server_url` (`http://192.168.1.200:5232/kalio`)
resolves from the container on the same host unchanged. No OAuth, no expiry, no
refresh. Encryption-at-rest is a later nicety; on a private single-user volume a
`0600` file is the MVP answer.

## Which writes are in

The **core** set only:

- Events: `create_event`, `update_event`, `delete_event_cmd`, `respond_to_event`
  (RSVP), and `refresh_event`.
- Tasks: `create_task`, `update_task`, `delete_task_cmd`, `set_task_completed`,
  and the list-management writes.
- Calendars: `set_calendar_selected`, `set_calendar_sync`, `set_calendar_color`,
  `set_calendar_label`.
- Settings: `set_setting`, `set_weather_location`, `set_display_timezone`.
- `sync_now` (forces a sync).

All of these end in the same `*_body` functions the CLI already calls; the
CalDAV ones are **etag-guarded** (`caldav_write.rs`), so a write that raced the
laptop tells you instead of overwriting — the same behaviour as the desktop.

**Out:** `connect_caldav`, `subscribe_webcal`, `sign_out` (account management);
`plan/run_ics_import`, `export_event` (file dialogs → upload/download);
notifications/reminders; and the desktop-only commands (`install_update`,
`restart_app`, `menubar_*`, `take_quick_add`, `take_preferences`). `open_conference`
(the Join link) is cheap and stays: the server resolves the URL and the client
opens it.

## The UI shim

One file, imported first in `ui/src/main.ts`, installs
`window.__TAURI_INTERNALS__` (modelled on Tauri's own `mocks`):

- `invoke(cmd, args)` → `POST /api/<cmd>`; resolve JSON on 2xx, throw the server's
  error string on non-2xx (Tauri rejects with the Rust `Err`'s `Display`).
- `plugin:event|listen`/`|unlisten` intercepted locally; the WebSocket dispatches
  into `__TAURI_INTERNALS__.runCallback`.
- `ui/dist` needs no change (no router, no SPA fallback); served at `/`.

The UI's write paths — the event form, drag, the pending-change queue, the task
checkboxes — all work over `invoke` and need no change.

## Deployment (on 192.168.1.200)

1. `docker compose up -d` with the image, a named volume at `/data`, and
   `TZ=Europe/Bucharest`, `XDG_DATA_HOME=/data`, binding `0.0.0.0:8080`.
2. Install Tailscale on the box; `tailscale up`. Join your phone to the tailnet.
3. Seed the credential once (above).
4. Open `http://<tailscale-name>:8080` from the phone — first boot syncs the
   whole calendar from Radicale.

Image, straight from research `03-minimal-rust-sqlite-image`: glibc build on
`rust:1-trixie`, run on `gcr.io/distroless/cc-debian13:nonroot` (~22 MB); bundled
SQLite + `webpki-roots` mean no SQLite/OpenSSL/cert files, `tzdata` the only
extra; non-root, exec-form `HEALTHCHECK` against `/healthz`. The server must not
depend on `src-tauri`, or Tauri/WebKitGTK get pulled in.

## Work breakdown

| # | Task | Size |
|---|---|---|
| 1 | `omacal-view`: move `commands.rs` + the read helpers, the write bodies and `sync_accounts`; replace `emit` with an event-sink trait and `keyring` with a credential-store trait; rewire `src-tauri`; keep tests green | L |
| 2 | `omacal-server`: axum, `AppState`, `/api/*` read **and** write routes, `/healthz`, `ServeDir`, stubbed `get_status`/`get_palette` | L |
| 3 | Credential store on the volume + the one-time import path | S |
| 4 | Sync ticker calling `sync_accounts` with the store's credential; WS broadcast | S–M |
| 5 | UI `main.ts` shim (`invoke`→fetch, `listen`→WS) | M |
| 6 | Dockerfile + compose (multi-stage, distroless, volume, `TZ`, healthcheck) | S |
| 7 | End-to-end on the box: seed credential, boot, verify the four views + writes + tasks + search from the phone over Tailscale | M |

Rough total: **~1.5–2 weeks**, the bulk in task 1 (the move) and task 2.

## Risks and open items

- **`omacal-view` is the real cost.** The move is mechanical but touches many
  files; the risk is a call site that secretly needs `AppHandle`. Contained by
  moving one module at a time with tests green between.
- **Time zone.** `TZ=Europe/Bucharest` on the container, or every day boundary is
  wrong. A compose line, not code.
- **Concurrent writers.** The laptop and the server both sync Radicale. CalDAV
  writes are etag-guarded, so no silent overwrite; Google had the same story.
- **No login is a deliberate bet** on Tailscale. If the tailnet is ever shared or
  the port exposed, add auth first.
- **Deferred, not forgotten:** account management, `.ics` import/export, and
  reminders. The invite tray will list invitations but answering needs
  `respond_to_event` (in scope) — reminder *notifications* are what is out.

## Sources

- `.scratch/web-app-mvp/issues/04-mvp-scope-decision.md` — the grilling result
- `.scratch/web-app-mvp/issues/01-read-only-server-surface.md` — the read analysis
- `.scratch/web-app-mvp/issues/03-ui-shim-surface.md` — the exact command/event surface
- `.scratch/web-app-docker/research/01-tauri-frontend-in-a-browser.md` — the shim
- `.scratch/web-app-docker/research/03-minimal-rust-sqlite-image.md` — the image
- `docs/superpowers/specs/2026-10-12-omacal-web-docker-feasibility.md`
