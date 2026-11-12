# The licence

How the server reads its licence, what each state of it changes, and how to install a new one. The short version: **a licence problem never stops a sale.** Holds, releases, bookings and the buyer's map work the same with a valid licence, an expired one, or none.

## Installing a licence

A licence is a small JSON file, signed by Cavea's issuer and checked by the server offline: the server never calls anywhere to check it. Give it to the server in one of two ways:

- **`CAVEA_LICENCE`**: the file's path, or the file's contents when the value starts with `{`. A path is read again every 30 s, so replacing the file installs a renewal without a restart. In Docker Compose, mount the file read-only into the server's container and set the variable to its path inside the container.
- **Uploaded** by a console user who owns every workspace on the server, through `PUT /console/api/licence` ([the API](../api/console.md#consoleLicence)). It is stored in PostgreSQL, so it is in your backups. A file that does not verify is refused and nothing is stored.

When both exist, the better one counts: valid, then in its grace period, then expired, then invalid. So uploading can never make things worse than the setting, and an old file on disk cannot hide a renewal that was uploaded. The server logs the licence's state at start and whenever it changes, with the licence's id and, when it is invalid, why; never the file itself.

> **Not yet** The console has no screen for the licence yet: upload it through the API with a console session, or use `CAVEA_LICENCE`.

## What each state changes

| State | Seat operations, buyers' maps, your backend's API calls | The console |
|---|---|---|
| Valid | Normal | Normal |
| Grace: expired, within the licence's grace days | Normal | Normal |
| Expired, past its grace | Normal | Changing charts and events (creating, editing, publishing, archiving, uploading images) is refused with `403`, `reason` `licence_expired`, until a renewal is installed. Selling, the event manager, keys, members and the audit log still work |
| Missing or invalid | Normal | Normal |

A secret key's calls are never refused for the licence, so a backend that creates tomorrow's event keeps selling. The public event information tells buyers' pages whether the server is licensed, and the domains it is licensed for, and nothing else about the licence.

## Before the first release

This version of the server trusts no signing key yet, so every licence file reads as invalid, and every server runs in the "missing or invalid" state, selling normally. Licences can be issued once Cavea's first release adds its key.
