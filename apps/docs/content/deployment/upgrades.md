# Upgrades and restarts

How to move to a new version, what happens to buyers while the server restarts, how it stops, and why one server serves one database today. By the end you will know how to upgrade without losing a hold or a booking.

## Upgrade

1. Read the release notes. A release whose database migration cannot run while the server serves says so there.
2. [Take a backup](backups.md#take-a-backup). Migrations only go forward: an older server does not run on a database a newer one has migrated, so the backup is the way back.
3. Change the image's tag in `compose.yaml`, then:

   ```sh
   docker compose pull server
   docker compose up -d --wait
   ```

   Compose stops the old server and starts the new one. The new server applies its migrations before it listens, and is ready when `docker compose up --wait` returns.

4. [Check the seat state](monitoring.md#checking-the-seat-state) with `cavea-server verify --all`.

## What buyers see during a restart

The server is unreachable from the moment it stops to the moment the new one is ready: seconds, mostly the time to stop (up to 20 s, below) and to start. Measured on the developers' machine while the server was killed under load and started again: it answered again 0.6 to 1.7 s after the kill, holds succeeded at once after that, and every buyer's live connection was back within 2 s.

- **Seat maps** stay on screen and usable. Their live connections reconnect by themselves, with a growing wait, and catch up on what they missed. A hold attempted while the server is down fails and the buyer sees a notice; they can tap again.
- **Holds** keep their expiry: a restart neither extends nor ends them, and those that expire meanwhile are freed when the server is back.
- **Your backend** gets `503` or no answer for a moment: retry bookings with the same idempotency key ([hold, pay, book](../guides/hold-pay-book.md#when-something-goes-wrong)).

Upgrade outside an on-sale.

## Stopping

On `SIGTERM` the server:

1. answers `/readyz` with `503`, so a load balancer stops sending it requests;
2. closes every live connection with a message telling the seat map to reconnect;
3. stops accepting connections and gives requests in flight up to 10 s to finish;
4. commits every seat command it accepted and releases each event it owns;
5. exits, within 20 s of the signal.

Give it 30 s before a forced kill: `stop_grace_period: 30s` in Compose (as in [Running Cavea](index.md)), `terminationGracePeriodSeconds` on Kubernetes. Docker's own default is 10 s, which can cut step 4 short. Even a forced kill loses nothing acknowledged: a hold or booking is answered only after its commit.

## One server per database

Run one server per database. Several servers sharing one database's load, and a standby taking over from a failed server, are planned and not built.

What exists already is the safety underneath them: each event is owned by one server at a time, through a lease in PostgreSQL that the owner renews (every 5 s when idle) and that lapses 15 s after it stops renewing (`CAVEA_HEARTBEAT_SECS`, `CAVEA_LEASE_SECS`). A second server on the same database, as during an upgrade whose new server starts before the old one has stopped, can never corrupt the seat state: it cannot commit to an event the other owns. But it cannot serve that event either until the owner releases it on stopping, or its lease lapses if it died. So a "rolling" restart with both servers taking traffic does not help: send traffic to one server at a time. Each server needs a distinct `CAVEA_NODE_ID` (the host name by default, which containers already make distinct), and during the overlap the database must allow both servers' connections (`CAVEA_DB_POOL` each).
