# Monitoring

How to tell that the server is healthy, what it measures, what to alert on, what its log says, and how to check that the seat state is consistent. By the end you will have probes, a Prometheus scrape and a short list of alerts.

## Health and readiness

| Route | Answers | Use it for |
|---|---|---|
| `GET /healthz` | `200 ok` while the process serves HTTP | Liveness: restart the container if it fails |
| `GET /readyz` | `200 ready`, or `503` with `database unreachable` or `shutting down` | Readiness: send traffic only while it is `200` |

Neither takes a credential, and neither says anything about a workspace. `/readyz` reads a database check the server makes once a second, so probing it often costs nothing; it turns `503` within about a second of PostgreSQL becoming unreachable, and from the moment the server is told to stop. The server applies its database migrations before it listens, so a server that answers is migrated. At start it waits up to 30 s (`CAVEA_DB_WAIT_SECS`) for PostgreSQL to accept a connection, logging `PostgreSQL is not reachable yet` with the reason once a try, and answers neither route until it does: a probe counts that as not ready. If the database is still not there, or refuses the password, the server stops with a message saying why. The image's health check runs `cavea-server probe`, which asks `/readyz` inside the container.

## Metrics

`GET /metrics` is Prometheus text. It needs the admin key, `Authorization: Bearer <CAVEA_ADMIN_KEY>`, because its figures cover every workspace on the server; without `CAVEA_ADMIN_KEY` set it refuses every request. In Prometheus, give the key in the scrape job's `authorization` section (`credentials`, or `credentials_file`) and scrape over your TLS address or the internal network.

<!-- metrics -->

Labels: `op` is the seat command (`hold`, `release`, `book`, …) and `result` is `ok` or the error code; `route` is the route's template (such as `/v1/events/{event_key}/book`) and `status` the HTTP status; `transport` is `websocket` or `sse`; `budget` is `holds`, `reads` or `sign_in`. No key, token, workspace, event or address is ever a label. Durations are histograms with buckets from 0.5 ms to 10 s.

### What to alert on

| Watch | Means | Look at |
|---|---|---|
| `/readyz` not `200` for more than 30 s | The database is unreachable, or the server is stuck stopping | The server's log, the database |
| `cavea_rate_limited_total{budget="holds"}` climbing during an on-sale | Buyers are refused holds: usually a proxy missing from `CAVEA_TRUSTED_PROXIES`, so everyone shares one budget | [Client addresses](proxy.md#client-addresses-and-rate-limits) |
| `cavea_commands_total` with `result` `unavailable` or `overloaded` | Holds and bookings are failing: the database is slow or down, or an event's queue is full | `cavea_commit_duration_seconds`, `cavea_actor_mailbox_depth`, the database |
| The 99th percentile of `cavea_command_duration_seconds{op="hold"}` above 100 ms | Buyers wait for their seats. The target is 15 ms | `cavea_commit_duration_seconds` (the database's writes) and the server's CPU |
| `cavea_db_pool_in_use` at `CAVEA_DB_POOL` for minutes | Requests queue for a database connection | Database load; the pool's size |
| `cavea_http_requests_total{status="500"}` above zero | A bug: the request id in the log finds it | The log, by request id |
| `cavea_rt_resyncs_total` climbing | Buyers' connections fall behind and are sent the whole state again | Network, CPU, the number of watchers |
| `cavea_log_records_dropped_total` climbing | The log output is full or failing; records are dropped rather than slowing the server | The log collector, the disk |
| `cavea_holds_active` far above what your sales explain | Possibly a script holding seats | [Bot protection](../guides/bot-protection.md) |

## Logs

The server writes structured records to standard output, one JSON object per line (`CAVEA_LOG_FORMAT=json`, the default), at the level `CAVEA_LOG` sets. Every request logs one `request completed` record with `request_id`, `method`, the matched `route`, `status` and `duration_ms`, and the `workspace` and `event` once known. Records made while serving a request carry its spans, so its request id.

Every answer carries its request id in the `Cavea-Request-Id` header, and error answers also in the body's `request_id`. When a customer reports a failed request, that id finds every record of it. A client may send its own id in the same header (1 to 64 letters, digits, `.`, `_` or `-`) to match its own logs.

Never in the log: API keys, hold tokens, session cookies, passwords, invitation links, query strings and request bodies, and the raw path (it can carry customer keys): the route's template is logged instead. The one exception is the setup link, below. Live connections log when they subscribe and when they close, with why; nothing per frame.

A panic in a request (a bug) answers that request `500` and logs one `error` record with its location; every other request goes on being served.

## Startup warnings

At start, before it connects to the database, the server logs one `warn` record, `insecure setting`, for each setting that is unsafe in production, with `setting` naming the variable and `why` saying what it exposes (never the value):

| Setting | Warned when |
|---|---|
| `CAVEA_PUBLIC_URL` | It is not `https://`, and not `localhost` or a loopback address |
| `CAVEA_CONSOLE_COOKIE_SECURE` | It is `false` |
| `CAVEA_ADMIN_KEY` | It is not set: creating workspaces and `/metrics` are refused |
| `CAVEA_RATE_LIMIT_HOLDS`, `CAVEA_RATE_LIMIT_READS`, `CAVEA_RATE_LIMIT_SIGN_IN` | One is `0` |
| `CAVEA_TRUSTED_PROXIES` | It trusts every address (`0.0.0.0/0` or `::/0`), so any client could choose the address its limits count against |

A production set-up like the one in [Running Cavea](index.md) logs none. The server still starts with these; it refuses to start only with a value it cannot use.

While the server has no user, it logs the console's one-time setup link at every start (`setup_link`), on purpose: it is how the first owner gets in. The link holds a token that creates the owner account, valid for an hour; a restart while there is still no user logs a new one. Anyone who can read those logs during that hour can create the owner first, so keep a fresh server's logs private, and create the owner at once; once it exists, no link is logged and an old one no longer works.

## Checking the seat state

`cavea-server verify` is the reconciler: it reads the database and checks, event by event, that the seat state the server serves is exactly what its log of commands says, and that no seat is held or booked twice. It only reads, so it is safe on a live server. Run it after an incident, a restore or an upgrade, or on a schedule. In the Docker Compose set-up the server's container has the database URL already:

```sh check=run:deployment
docker compose exec server cavea-server verify --all
```

`--workspace <key> --event <key>` checks one event; `--json` prints the result as JSON. It exits `0` when everything matches, `1` when it found a difference (each is printed), `2` for a usage error and `3` when it could not run. A difference is a bug: keep the output and report it.
