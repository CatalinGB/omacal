# OmaCal Web MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** OmaCal's real UI served from a container on the Radicale box
(192.168.1.200), reachable from a phone over Tailscale, reading **and writing**
your one CalDAV account — no Google, no login.

**Architecture:** Move the Tauri-free service layer (`src-tauri`'s read
assembly, write bodies and `sync_accounts`) into a new `omacal-view` crate that
`src-tauri` and a new `omacal-server` both depend on. The server is an axum skin
over that crate — one route per command, a WebSocket for `sync-finished`, and
`ServeDir` for the static Svelte build. The UI is unchanged except for one
`main.ts` shim that repoints `invoke` at `fetch` and `listen` at the WebSocket.
The desktop app's own behaviour does not change.

**Tech Stack:** Rust (axum, tokio, tower-http, sqlx/SQLite bundled,
reqwest/rustls), Svelte 5 + Vite (no change), Docker (multi-stage, distroless),
Tailscale.

**Spec:** `docs/superpowers/specs/2026-10-12-omacal-web-mvp-scoping.md`
**Research:** `.scratch/web-app-mvp/`, `.scratch/web-app-docker/research/`

## Global Constraints

- **Fork only.** Every commit goes to the fork (`CatalinGB/omacal`); never push
  to `origin` (`x3me/omacal`), never open a PR upstream. Implementation proceeds
  on the fork branch (`research/web-app-mvp`, or a `feat/web-app-mvp` off it).
- **The desktop app does not change behaviour.** `src-tauri` must stay green
  (`cargo test --workspace`) after every task; no read or write payload changes.
- **CalDAV only.** No OAuth, no consent screen, no Google paths. The account is
  Radicale at `http://192.168.1.200:5232/kalio`, user `kalio`.
- **`omacal-server` must not depend on `src-tauri`.** If it does, Tauri/WebKitGTK
  get pulled in and the distroless image is impossible.
- **Container:** `TZ=Europe/Bucharest`, `XDG_DATA_HOME=/data`, bind
  `0.0.0.0:8080`, run non-root, DB on a named volume.
- **Core writes only.** Account management, `.ics` import/export and
  notifications are out of scope; do not build them.
- **House rules.** Every new test is proven red against deliberately broken code
  before it is trusted (`docs/testing-standard.md`). Before pushing run
  `cargo clippy --workspace --all-targets -- -D warnings` and
  `npm --prefix ui run check`. Run Playwright as `ui/node_modules/.bin/playwright`
  from `ui/`, never bare `npx`.

## Sequencing

```
1  extract read layer ──┐
2  extract write+sync ──┼──▶ 4  sync live ──▶ 5  event writes ──┐
3  server skeleton ─────┘                            │          ├──▶ 7 harden + E2E
                                                     6  task/calendar/settings ┘
```

Task 3's skeleton can start before 1; its read routes need 1. Task 4 needs 2 and
3. Tasks 5 and 6 need 4 and can go in parallel. Task 7 needs 5 and 6.

## Review Focus

1. **No silent behaviour drift in the extraction.** Task 1's acceptance is that
   the existing workspace tests pass unchanged; a diff that "fixes" a payload
   shape is a bug, not a cleanup.
2. **The seams, not `AppHandle`.** After Task 2 every `emit(...)` in a body goes
   through the `EventSink` trait and every `keyring` call through
   `CredentialStore` — the server implements both, the desktop keeps its own.
3. **The shim's error shape.** `invoke` rejects with the Rust `Err`'s `Display`;
   the shim must throw the server's error string on non-2xx or the UI's existing
   `.catch` paths change behaviour (`render-errors` in the event form, refusal
   banners).
4. **Concurrency.** The laptop and the server both write Radicale. CalDAV writes
   are etag-guarded; a task that removes or weakens that guard is a regression.

---

### Task 1: Extract the read layer into `omacal-view`

**Goal / delivers:** A new `crates/omacal-view` holding the pure read assembly so
a Tauri-free caller can answer the view commands. `src-tauri` still answers
everything exactly as before.

**Blocked by:** None (can start immediately).

**Files:**
- Create: `crates/omacal-view/` (Cargo.toml, `src/lib.rs`, modules).
- Move into it: the assemblers from `src-tauri/src/commands.rs`; the read bodies
  from `src-tauri/src/lib.rs` (`get_days_impl`, the `get_month`/`get_year` logic,
  `get_big_year_impl`, `display_tz`, `now_ms`); the read half of
  `src-tauri/src/settings.rs` (`read_settings`, `combine_identical_events`,
  `appearance`).
- Modify: `src-tauri` call sites and `Cargo.toml`.

- [ ] `crates/omacal-view` builds with no `tauri` in its dependency tree.
- [ ] `src-tauri`'s read commands produce byte-identical payloads (existing
      tests pass unchanged).
- [ ] `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` green.
- [ ] No write path is touched in this task.

**Verification:** `cargo test --workspace`; a diff of the moved code is a move.

---

### Task 2: Extract the write and sync layers; add the two seams

**Goal / delivers:** The `*_body` write functions and the sync orchestration live
in `omacal-view`, taking their two desktop dependencies as traits: an
`EventSink` (replacing `AppHandle::emit`) and a `CredentialStore` (replacing
`keyring`). `src-tauri` supplies the desktop implementations and behaves
unchanged.

**Blocked by:** Task 1.

**Files:**
- Move into `omacal-view`: the write bodies from `events.rs`, `tasks.rs`,
  `calendars.rs`, `settings.rs`, `caldav_write.rs`; `sync_accounts`,
  `calendars_to_sync`, `access_token_for`; the CalDAV credential read from
  `caldav_account.rs`.
- Add: `EventSink` and `CredentialStore` traits (in `omacal-view`), with desktop
  impls in `src-tauri`.
- Modify: `src-tauri` call sites; `ipc.rs`'s `dispatch` continues to work.

- [ ] No `emit(` or `keyring::` call remains inside `omacal-view`.
- [ ] The CLI write round-trip (`ipc.rs`'s socket test) still passes.
- [ ] `cargo test --workspace` and clippy green; the desktop writes are unchanged.

**Verification:** workspace tests; the existing `ipc` e2e test.

---

### Task 3: Server skeleton, UI shim and container — the app opens in a browser

**Goal / delivers:** `http://<box>:8080` loads the real Svelte UI in a browser
over Tailscale. The pipe from static asset → shim → HTTP route → `omacal-view`
works end to end for the boot commands.

**Blocked by:** Task 1 (for the read routes; the static skeleton needs no
extraction).

**Files:**
- Create: `crates/omacal-server/` (axum, `AppState`, routes, `/healthz`,
  `tower-http` `ServeDir`).
- Create: `ui/src/lib/tauri-web-shim.ts`; modify `ui/src/main.ts` to import it
  first.
- Create: `Dockerfile`, `docker-compose.yml` (dev-level image is fine here).

- [ ] `ui/src/main.ts` installs `window.__TAURI_INTERNALS__` before Svelte
      mounts; no `invoke` throws a raw `TypeError`.
- [ ] `GET /healthz` → 200; `POST /api/get_settings` → the same shape the desktop
      returns; `GET /` serves `ui/dist`.
- [ ] In a browser, the app boots, the header renders, and `get_status` /
      `get_settings` / `get_calendars` answer (the grid may be empty until sync).
- [ ] The server binary has no `src-tauri` dependency in `cargo tree`.

**Verification:** a server smoke test for `/healthz` and one route; a manual
browser load over Tailscale.

---

### Task 4: Sync live — credential import, ticker, WebSocket

**Goal / delivers:** With one imported CalDAV password, the server syncs your
Radicale account on boot and on a timer, and the browser shows your **real**
calendar, refreshing in place when a sync lands.

**Blocked by:** Tasks 2 and 3.

**Files:**
- Add: a credential store on the volume (`0600`, from `CredentialStore`) and the
  one-time import path.
- Add: the sync ticker (task on `sync_loop::interval_ms`) calling `sync_accounts`;
  a WebSocket that broadcasts `sync-finished` / `sync-failed`.
- Modify: the shim to dispatch WebSocket messages into
  `__TAURI_INTERNALS__.runCallback`.
- Add read routes for the day/month/year commands.

- [ ] First boot pulls the calendar from Radicale; the browser shows real events.
- [ ] A sync landing updates the open page without a manual reload
      (`sync-finished` over the WebSocket).
- [ ] The credential file is `0600`, on the volume, and absent from the image.
- [ ] No OAuth code path is reachable.

**Verification:** on the box, over Tailscale — see real events; `touch` the DB
and watch a forced sync repaint.

---

### Task 5: Event writes

**Goal / delivers:** Create, move, delete and RSVP to events from the phone, and
open a meeting's Join link — writing through to Radicale with the same guards as
the desktop.

**Blocked by:** Task 4.

**Files:**
- Add routes: `create_event`, `update_event`, `delete_event_cmd`,
  `respond_to_event`, `refresh_event`, `event_detail`, `open_conference`.
- Wire the event sink so a successful write emits the reload signal the UI
  already listens for.

- [ ] Creating an event in the browser lands in Radicale (visible from the laptop
      within one sync).
- [ ] A move/delete/RSVP from the phone persists; the UI's pending-change queue
      behaves as on the desktop.
- [ ] A racing edit reports the etag conflict instead of overwriting.
- [ ] `open_conference` resolves the URL and the client opens it.

**Verification:** manual from the phone; cross-check on the laptop; a two-writer
race forced by hand.

---

### Task 6: Task, calendar and settings writes

**Goal / delivers:** The rest of the core writes — the Tasks pane, the calendar
list's visibility/colour/label toggles, and app settings — work from the browser.

**Blocked by:** Task 4 (may run in parallel with Task 5).

**Files:**
- Add routes for the task writes and list management; `set_calendar_selected`,
  `set_calendar_sync`, `set_calendar_color`, `set_calendar_label`;
  `set_setting`, `set_weather_location`, `set_display_timezone`.
- Stub/defer `connect_caldav`, `subscribe_webcal`, `sign_out`, import/export.

- [ ] Ticking a task, adding a task, and toggling a calendar persist.
- [ ] Changing week-start / appearance from the phone reflects after reload.
- [ ] The deferred commands return a clear "not available in the web build"
      refusal rather than a raw error.

**Verification:** manual from the phone; the deferred refusals checked by hand.

---

### Task 7: Harden and deploy end-to-end

**Goal / delivers:** The distroless image, compose file and a short README that
takes a fresh box to a working instance; one end-to-end pass from the phone.

**Blocked by:** Tasks 5 and 6.

**Files:**
- Finalise `Dockerfile` (multi-stage: node → `ui/dist`; `rust:1-trixie` with
  cargo-chef → server binary; `gcr.io/distroless/cc-debian13:nonroot` runtime).
- Finalise `docker-compose.yml` (volume at `/data`, `TZ`, `0.0.0.0:8080`,
  healthcheck, restart policy).
- Add a short deploy README.

- [ ] `docker compose up -d` on `192.168.1.200` yields a working instance.
- [ ] The container restarts cleanly and survives a reboot; the volume holds the
      DB and the credential.
- [ ] The image contains no secret; `docker history` shows no plaintext token.
- [ ] Reachable only over Tailscale (not the open internet).
- [ ] A full pass from the phone: view, create, move, delete, tick a task, search.

**Verification:** a clean deploy on the box; the phone pass.
