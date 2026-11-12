# Running Cavea

This section is for the person who runs the Cavea server. By the end of this page you will have the server, its PostgreSQL database and a TLS proxy running under Docker Compose on one machine, reachable at your own HTTPS address, and know what each part needs.

The other pages go further: [settings](settings.md), [the reverse proxy and rate limits](proxy.md), [monitoring](monitoring.md), [backups](backups.md), [upgrades and restarts](upgrades.md) and [the licence](licence.md).

## What runs

| Part | What it is |
|---|---|
| The server | One container, `{{image}}`: the API, the realtime connections, the console and the designer, in one binary. It keeps no files: everything it stores is in PostgreSQL |
| PostgreSQL | Version 15 or later. Every chart, event, seat, hold, booking, key, member, uploaded image and the audit log |
| A TLS proxy | Terminates HTTPS and passes WebSocket upgrades: Caddy below, or your load balancer |

The image runs as an unprivileged user with no shell, listens on port 8080, and works with a read-only root filesystem. It is built for x86-64 and arm64 Linux.

> **Not yet** The image is not published. Until it is, build it from the repository: [Before the first release](../before-release.md#build-the-server-image).

## Sizing

One server with one PostgreSQL serves every workspace and event; running several servers on one database is not supported yet ([upgrades and restarts](upgrades.md#one-server-per-database)).

| | Server | PostgreSQL |
|---|---|---|
| Smallest | 1 vCPU, 512 MB of memory | Version 15, any small instance |
| For on-sales | 4 vCPU, 8 GB | On fast storage, in the same network as the server |

Those are starting points, not measured capacities: **no measurement on reference hardware exists yet.** What has been measured is on one developer's desktop (AMD Ryzen 9 7900X; the server limited to 4 threads; PostgreSQL 18 in Docker on the same machine and its shared SSD; every client over loopback), so it shows how the server behaves, not what your hardware will do:

- **Correctness held under every load tried**: no seat held twice, every acknowledged hold in the log, the [reconciler](monitoring.md#checking-the-seat-state) clean, after 600 s of 2,000 buyers arriving a second, after `kill -9` of the server during load, and after PostgreSQL restarts during load.
- **55,000 open live connections** on one server (50,000 watchers and 5,000 buyers) were held without a drop, at about 1.2 GB of memory. But with that many watchers a hold took about 1 s at the 99th percentile, against a target of 15 ms: sending each change to 55,000 connections competed with requests for the 4 threads. With 5,000 watchers, 22 ms.
- **2,000 holds a second** with 100 watchers: 23 ms at the 99th percentile against a private database, bound by the database's commits on that shared disk.
- **Memory stayed flat** over 10 minutes of sustained load, at about 265 MB.

So: give the server CPU in proportion to the number of buyers watching the map at once, and put PostgreSQL on storage with fast synchronous writes, since every hold waits for its commit.

## A production set-up with Docker Compose

This runs the server, PostgreSQL and Caddy, which gets a certificate for your host name from Let's Encrypt and renews it. You need a machine with Docker and Docker Compose, a DNS name pointing at it (`seats.example.com` here), and ports 80 and 443 open to the internet.

1. Make a directory for it, and in it save this as `compose.yaml`:

   ```yaml file=compose.yaml check=run:deployment
   name: cavea

   services:
     postgres:
       image: postgres:18-alpine
       # Every hold is acknowledged only once its commit is durable.
       command: ["postgres", "-c", "synchronous_commit=on"]
       environment:
         POSTGRES_USER: cavea
         POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?}
         POSTGRES_DB: cavea
       volumes:
         - postgres-data:/var/lib/postgresql
       healthcheck:
         test: ["CMD-SHELL", "pg_isready -U cavea -d cavea"]
         interval: 2s
         timeout: 5s
         retries: 30
       restart: unless-stopped

     server:
       image: {{image}}
       depends_on:
         postgres:
           condition: service_healthy
       environment:
         CAVEA_DATABASE_URL: postgres://cavea:${POSTGRES_PASSWORD}@postgres:5432/cavea
         CAVEA_PUBLIC_URL: https://seats.example.com
         CAVEA_ADMIN_KEY: ${CAVEA_ADMIN_KEY:?}
         CAVEA_TRUSTED_PROXIES: 172.31.250.0/24
       read_only: true
       # More than the server's 20 s shutdown.
       stop_grace_period: 30s
       restart: unless-stopped

     caddy:
       image: caddy:2.10-alpine
       depends_on:
         server:
           condition: service_healthy
       ports:
         - "80:80"
         - "443:443"
       volumes:
         - ./Caddyfile:/etc/caddy/Caddyfile:ro
         - caddy-data:/data
       restart: unless-stopped

   networks:
     default:
       ipam:
         config:
           - subnet: 172.31.250.0/24

   volumes:
     postgres-data:
     caddy-data:
   ```

   The server's port is not published: only Caddy reaches it. The fixed subnet lets the server trust Caddy's `X-Forwarded-For` header, so that rate limits count each buyer rather than the proxy ([client addresses](proxy.md#client-addresses-and-rate-limits)); if that range is taken on your machine, pick another private one and change both lines.

2. Save this as `Caddyfile` beside it, with your host name:

   ```caddyfile file=Caddyfile check=run:deployment
   seats.example.com {
   	reverse_proxy server:8080
   }
   ```

   Caddy passes WebSocket upgrades and streams Server-Sent Events as they come, so nothing else is needed. [The reverse proxy](proxy.md) says what any other proxy must do.

3. Make the two secrets, and keep them somewhere safe: the database password goes into the database URL as it is, so use letters and digits only, and the admin key is the operator's own credential (32 to 512 characters).

   ```sh
   cat > .env <<EOF
   POSTGRES_PASSWORD=$(openssl rand -hex 32)
   CAVEA_ADMIN_KEY=$(openssl rand -hex 32)
   EOF
   chmod 600 .env
   ```

   Docker Compose reads `.env` from the directory. A licence, if you have one, is added later ([the licence](licence.md)): the server runs and sells without one.

4. Start it:

   ```sh check=run:deployment
   docker compose up -d --wait
   ```

   It returns when PostgreSQL, the server and Caddy are up. The server creates its tables on first start.

5. Print the one-time **setup link** and open it in your browser to create the owner account:

   ```sh check=run:deployment
   docker compose logs server | grep -o 'https://[^"]*/console/setup#token=[0-9a-f]*'
   ```

   It is valid for an hour; restarting the server makes a new one while there is no user. The console is at `https://seats.example.com/console`, the API at `https://seats.example.com/v1`.

The server warns in its log at start about any setting that is unsafe in production; this set-up has none ([startup warnings](monitoring.md#startup-warnings)).

## Running it another way

The server is one container and one environment: any platform that runs containers runs it (Kubernetes, ECS, Nomad, a systemd unit running Docker). What it needs:

- `CAVEA_DATABASE_URL`, and a PostgreSQL 15 or later it can create tables in. A managed PostgreSQL works; leave `synchronous_commit` on.
- A TLS proxy or load balancer in front, which passes WebSocket upgrades ([the reverse proxy](proxy.md)).
- A readiness probe on `GET /readyz`, and 30 s to stop ([monitoring](monitoring.md#health-and-readiness), [shutdown](upgrades.md#stopping)). The image's own health check runs `cavea-server probe`, which asks `/readyz` inside the container.
- One server per database ([why](upgrades.md#one-server-per-database)).

Helm charts and a systemd unit are not provided yet.
