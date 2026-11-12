# Backups and restore

What to back up, how to take a backup of the Docker Compose set-up, and how to restore it. By the end you will have a backup command to schedule and a restore you have tried. The documentation's own check runs both on this page's commands: a backup, the database deleted, the restore, then the bookings and an uploaded image checked.

## What to back up

| What | Where it lives | How to back it up |
|---|---|---|
| Charts, events, seat state, holds, bookings, API keys, members, the audit log, **uploaded images**, a licence uploaded through the console | PostgreSQL, all of it | A database backup (below) |
| The settings: the database password, `CAVEA_ADMIN_KEY` | Your `.env` file or secret store | Keep a copy in your password manager or secret store |
| A licence given in `CAVEA_LICENCE` | The file you named, or the variable | Keep the file you were sent |
| Caddy's certificates | The `caddy-data` volume | Nothing: Caddy obtains new ones |

The server itself keeps nothing on disk: PostgreSQL is the only thing whose loss loses data.

## Take a backup

From the directory of the [Docker Compose set-up](index.md#a-production-set-up-with-docker-compose):

```sh check=run:deployment
docker compose exec -T postgres pg_dump -U cavea --format=custom cavea > cavea-backup.dump
```

That is a consistent copy of the whole database while the server runs; it does not stop sales. Name each file by its date, keep them off the machine, and schedule it (a cron job or a systemd timer) as often as you can afford to lose bookings: a booking made after the last backup is not in it.

For no loss at all, use PostgreSQL's continuous archiving and point-in-time recovery, or a managed PostgreSQL that does it for you. That is the database's feature, and Cavea needs nothing special from it.

## Restore

A restore replaces everything in the database with the backup. Check the file first, while the database is still running: this lists what the backup holds, table by table.

```sh check=run:deployment
docker compose exec -T postgres pg_restore --list < cavea-backup.dump
```

1. Stop everything, and delete the database's volume. This deletes the current data: only do it once you are sure of the backup.

   ```sh check=run:deployment
   docker compose down
   docker volume rm cavea_postgres-data
   ```

2. Start PostgreSQL alone, which creates an empty database:

   ```sh check=run:deployment
   docker compose up -d --wait postgres
   ```

3. Restore into it:

   ```sh check=run:deployment
   docker compose exec -T postgres pg_restore -U cavea -d cavea --no-owner --exit-on-error < cavea-backup.dump
   ```

4. Start the server and Caddy:

   ```sh check=run:deployment
   docker compose up -d --wait
   ```

5. Check the seat state, as after any incident ([the reconciler](monitoring.md#checking-the-seat-state)):

   ```sh check=run:deployment
   docker compose exec server cavea-server verify --all
   ```

On a new machine, copy the compose file, the `Caddyfile` and the `.env` first, and start at step 2.

## After a restore

- **Bookings made after the backup are gone** from Cavea, though your orders have them. Compare your orders since the backup's time with the event's booked seats (by `order_ref`), and book again, or refund, what is missing.
- **Holds in the backup** come back with their old expiry; those already past it expire as soon as the server loads their event.
- **Buyers' pages** left open during the restore reconnect by themselves; a reload shows the restored state for certain.
- **API keys, members and sessions** are as they were at the backup: a key created after it no longer works.
