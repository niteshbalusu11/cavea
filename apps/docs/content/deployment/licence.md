# The licence

How the server reads its licence, what each state of it changes, and how to install a new one. The short version: **a licence problem never stops a sale.** Holds, releases, bookings and the buyer's map work the same with a valid licence, an expired one, or none.

## Installing a licence

A licence is a small JSON file, signed by Cavea's issuer and checked by the server offline: the server never calls anywhere to check it. Give it to the server in one of two ways:

- **`CAVEA_LICENCE`**: the file's path, or the file's contents when the value starts with `{`. A path is read again every 30 s, so replacing the file installs a renewal without a restart. In Docker Compose, mount the file read-only into the server's container and set the variable to its path inside the container.
- **Uploaded** in the console, under **Settings**, **Licence**: **Upload a licence file**, or paste its text, then **Upload licence**. Only a console user who owns every workspace on the server can (anyone else sees the state and whom to ask). It is stored in PostgreSQL, so it is in your backups, and every server on the database reads it within 30 s. A file that does not verify is refused, with the reason, and nothing is stored. The API route is [`PUT /console/api/licence`](../api/console.md#consoleLicence).

When both exist, the better one counts: valid, then in its grace period, then expired, then invalid. So uploading can never make things worse than the setting, and an old file on disk cannot hide a renewal that was uploaded. The server logs the licence's state at start and whenever it changes, with the licence's id and, when it is invalid, why; never the file itself. The console's **Licence** screen shows the state, and for whoever manages the server the licence's details and where it came from.

## What each state changes

| State | Seat operations, your backend's API calls | The console | Buyers' seat maps |
|---|---|---|---|
| Valid | Normal | Normal | No mark on the licensed domains (and on `localhost`); "Development licence" on any other site |
| Grace: expired, within the licence's grace days | Normal | A banner for owners and admins, with the last day of grace | As valid |
| Expired, past its grace | Normal | A banner; changing charts and events (creating, editing, publishing, archiving, uploading images) is closed, with the reason, until a renewal is installed. Selling, the event manager, keys, members and settings still work | A small "Powered by Cavea" mark |
| Missing or invalid | Normal | A one-line "Development licence" notice | A small "Development licence" mark |

A secret key's calls are never refused for the licence, so a backend that creates tomorrow's event keeps selling. The mark is small text in a corner of the map, decided once after the map is drawn: it never delays the map or takes a click. The public event information tells buyers' pages whether the server is licensed, and the domains it is licensed for, and nothing else about the licence. The console's own maps (the event manager, the designer's preview) never carry a mark.

## Before the first release

This version of the server trusts no signing key yet, so every licence file reads as invalid, and every server runs in the "missing or invalid" state, selling normally. Licences can be issued once Cavea's first release adds its key.
