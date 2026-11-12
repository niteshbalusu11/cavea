# Settings

Every setting of the server: an environment variable starting `CAVEA_`, its default, and what it does. Only `CAVEA_DATABASE_URL` is required. The site's build checks this table against the variables the server reads, so a setting cannot be missing from it.

A value the server cannot use (a malformed URL, a number out of range, an admin key that is too short) stops it at start with a message naming the variable, before it touches the database. It never ignores a bad value.

| Variable | Default | What it does |
|---|---|---|
| `CAVEA_DATABASE_URL` | None: required | The PostgreSQL connection URL, such as `postgres://cavea:password@db.internal:5432/cavea`. The server creates and migrates its tables in that database at start |
| `CAVEA_PUBLIC_URL` | None | The address people reach the server at, such as `https://seats.example.com`, without a trailing slash. The console's setup and invitation links start with it (without it, the setup link is only a path). When it is `https://`, every answer carries `Strict-Transport-Security`. When it is `http://` and not `localhost`, the server warns at start |
| `CAVEA_ADMIN_KEY` | None | The operator's key, 32 to 512 printable characters, for creating workspaces (`/v1/workspaces`) and reading `/metrics`. Unset, those refuse every request, and the server warns at start. Shorter than 32 characters, the server does not start |
| `CAVEA_LISTEN` | `0.0.0.0:8080` | The address and port the server listens on |
| `CAVEA_LOG` | `info` | Which log records are written, as a filter such as `info` or `info,cavea_server=debug` |
| `CAVEA_LOG_FORMAT` | `json` | `json`, one object per line, for production; `text`, readable lines, for development |
| `CAVEA_DB_POOL` | `16` | Connections to PostgreSQL, all opened at start and kept open. Count them against the database's `max_connections`, twice over while a restart overlaps the old and new server. A small server beside a small database does well with 4 |
| `CAVEA_TRUSTED_PROXIES` | None | Addresses and CIDR ranges of the proxies in front of the server, comma-separated, such as `10.0.0.0/8,2001:db8::/32`. Only from these is `X-Forwarded-For` believed, to find the buyer's address for rate limits ([the reverse proxy](proxy.md#client-addresses-and-rate-limits)). Trusting every address (`0.0.0.0/0`, `::/0`) warns at start |
| `CAVEA_RATE_LIMIT_HOLDS` | `120` | Hold-token creations, holds and releases from buyers' pages, per minute, per publishable key and client address. `0` turns it off, and warns at start |
| `CAVEA_RATE_LIMIT_READS` | `3000` | Every other request with a publishable key (the seat state, live connection attempts), per minute, per publishable key and client address. `0` turns it off, and warns at start |
| `CAVEA_RATE_LIMIT_SIGN_IN` | `10` | Console sign-ins, the setup and password changes, per minute, per client address. `0` turns it off, and warns at start |
| `CAVEA_RT_MAX_CONNECTIONS` | `60000` | Live connections (WebSocket and Server-Sent Events) the server holds open at most; beyond it, a new one is refused with `503` |
| `CAVEA_RT_MAX_PER_IP` | `200` | Live connections one client address may hold open; beyond it, `429`. High enough for many buyers behind one office or mobile network |
| `CAVEA_CORS_ORIGINS` | `*` | The web origins buyers' pages may call the public routes from: `*`, or a comma-separated list such as `https://tickets.example.com,https://www.example.com`. A publishable key can narrow this further with its own allowed origins |
| `CAVEA_SNAPSHOT_MAX_AGE` | `1` | Seconds a shared cache, such as a CDN, may serve the public seat state as fresh |
| `CAVEA_ASSET_MAX_BYTES` | `10485760` | The largest image `POST /v1/assets` takes (the designer's uploads), in bytes (10 MiB); at most 268435456 (256 MiB), since an upload is held in memory whole |
| `CAVEA_CONSOLE_COOKIE_SECURE` | `true` | `false` lets the console's session cookie travel over plain HTTP, for a development server reached by a host name other than `localhost`. Never in production; it warns at start |
| `CAVEA_LICENCE` | None | The licence: the path of the licence file, or its contents when the value starts with `{`. A path is read again every 30 s ([the licence](licence.md)) |
| `CAVEA_NODE_ID` | The host name | This server's name in the database's record of which server owns each event. Must not be empty |
| `CAVEA_LEASE_SECS` | `15` | How long a server's ownership of an event lasts unless it renews it ([one server per database](upgrades.md#one-server-per-database)) |
| `CAVEA_HEARTBEAT_SECS` | `5` | How often an idle event's ownership is renewed. Must be less than `CAVEA_LEASE_SECS` |

## Secrets

`CAVEA_DATABASE_URL`, `CAVEA_ADMIN_KEY` and `CAVEA_LICENCE` hold secrets or a customer's licence. Give them to the container from your platform's secret store or an `.env` file only its owner can read, never in a compose file you commit. The server never writes a secret to its log: not these, not API keys, hold tokens, session cookies or request bodies.
