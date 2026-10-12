# Deploying the OmaCal web app on the Radicale box

The target is the always-on box at **`192.168.1.200`** (x86_64 Linux, Docker
installed). It also runs Radicale at `http://192.168.1.200:5232/kalio`
(user `kalio`). OmaCal runs in a container there and is reached from your phone
over **Tailscale** — there is no app login, the tailnet is the auth.

The image is built from source on the box; nothing is published to a registry.

## Prerequisites

- Docker Engine with the Compose plugin (`docker compose version`).
- The Tailscale client (installed in step 4).
- **The database.** The container reads `omacal.db` from its volume, and that
  database must already contain your CalDAV account row (the one your desktop
  OmaCal created when you connected Radicale). If the box has never run OmaCal,
  copy the desktop's `omacal.db` — with its `-wal`/`-shm` sidecars, or a
  consistent snapshot (`sqlite3 omacal.db ".backup backup.db"`) — into the
  volume **after** the first `up` (step 2), under
  `/data/com.omacal.app/omacal.db`. A fresh, account-less database will serve an
  empty calendar and never sync.

## 1. Get the code

```sh
git clone https://github.com/CatalinGB/omacal.git
cd omacal
git checkout research/web-app-mvp   # the branch that carries the web build
```

## 2. Build and start

```sh
docker compose up -d --build
docker compose ps        # STATUS should reach "healthy"
```

This builds the Svelte UI, the `omacal-server` binary and the distroless image
(no shell, runs as uid 65532), then starts it with a named volume at `/data`.
`restart: unless-stopped` brings it back after a reboot.

The first boot also creates `/data/com.omacal.app/` (mode `0700`) for the
database and the credential file. If you are seeding a database from the
desktop, copy it in now:

```sh
docker compose cp /path/to/omacal.db omacal:/data/com.omacal.app/omacal.db
docker compose restart omacal
```

## 3. Seed the CalDAV credential once

The password is **not** baked into the image and is never passed as a build
argument. The container has no desktop keyring, so the web build reads a `0600`
JSON file on the volume; the one-shot `set-credential` subcommand writes it
without printing the secret:

```sh
printf '%s' "$PASSWORD" | docker compose exec -T omacal \
  /usr/local/bin/omacal-server set-credential caldav:kalio
docker compose restart omacal     # forces an immediate first sync
```

`caldav:kalio` is the account key (`caldav:<name>`, where `<name>` is the
account's address in OmaCal). The command prints only
`stored credential for caldav:kalio`.

<details>
<summary>Alternative: seed <code>credentials.json</code> by hand</summary>

The file is a flat JSON object at `/data/com.omacal.app/credentials.json`, mode
`0600`, owned by uid 65532. After the first boot (so the directory exists):

```sh
umask 077
printf '{"caldav:kalio":"%s"}\n' "$PASSWORD" > /tmp/credentials.json
sudo chown 65532:65532 /tmp/credentials.json
docker compose cp /tmp/credentials.json omacal:/data/com.omacal.app/credentials.json
rm -f /tmp/credentials.json
docker compose restart omacal
```

</details>

## 4. Install Tailscale and join the tailnet

```sh
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up           # authenticate in the browser it prints
tailscale status            # note the box's MagicDNS name
```

Also install Tailscale on the phone and sign in to the same tailnet.

## 5. Open it from the phone

Browse to the box's tailnet name on port 8080:

```
http://<tailnet-name>:8080
```

The port is published only on the host, which is itself reachable only inside
the tailnet — there is no public exposure and no port-forward needed.

## What lives where

- **Volume `omacal-data` → `/data`** holds, under `com.omacal.app/`:
  - `omacal.db` (plus `-wal`/`-shm`) — the calendar database;
  - `credentials.json`, mode `0600` — the CalDAV password, and nothing else.
- **The image contains no secret.** `.dockerignore` excludes `credentials.json`
  and `*.db*` from the build context, and the credential is only ever written
  into the volume at runtime. `docker history omacal-web` shows no token.

Back up the volume (or at least `omacal.db`) to keep the credential.

## Updating / operating

```sh
git pull
docker compose up -d --build     # rebuild and replace the container
docker compose logs -f omacal    # follow the server log
docker inspect --format '{{.State.Health.Status}}' $(docker compose ps -q omacal)
```

The `HEALTHCHECK` runs the binary's own `healthcheck` subcommand — a `GET
/healthz` against the bind address — and needs no shell or curl in the image.
