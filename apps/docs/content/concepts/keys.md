# Keys

The server API has three kinds of key, each for one place. This page says which goes where, how to restrict and rotate them, and what the console session adds.

| Key | Looks like | Belongs in | Can |
|---|---|---|---|
| [Secret key](#secret-keys) | `sk_` and 64 hex digits | Your backend only | Everything in its workspace |
| [Publishable key](#publishable-keys) | `pk_` and 64 hex digits | The buyer's page or app | Read an event's public seat state, create hold tokens, hold and release with them |
| [Admin key](#the-admin-key) | Whatever the operator sets | The operator's tools only | Create and list workspaces; read the server's metrics |

Send a key as `Authorization: Bearer <key>`. A key of the wrong kind for a route is `403 forbidden` (`reason` `key_kind`); an unknown or revoked key is `401 unauthorized`.

## Secret keys

A secret key acts for its whole [workspace](workspaces.md): charts, events, bookings, other keys. Keep it on your servers, in your secrets store; never put it in a page, an app or a repository.

Create one in the console (**Developers**, **New API key**, **Secret key**) or with the API ([create a key](../api/keys.md#createKey)). The key is shown once, at creation: Cavea keeps only its hash and its first characters, to show which key is which. Each key also has an id, `key_` and 32 hex digits, which is not secret: use it to revoke the key, and to name it in a [session token](session-tokens.md).

## Publishable keys

A publishable key is made to be public. It can read what any buyer may see and hold seats under hold tokens, within the event's limits, and nothing else: it can never book, block or change a sold seat.

A publishable key may list **allowed origins**, up to 50, such as `https://tickets.example.com`. A browser request from any other origin is refused (`403`, `reason` `origin`). A request with no `Origin` header is accepted, since apps send none and a script can send any; origins stop other websites from using your key in their pages, not a determined script. A key with no list accepts every origin. The list can be changed later ([change a key's origins](../api/keys.md#updateKeyOrigins)).

## The admin key

The admin key belongs to whoever runs the server, set in its environment as `CAVEA_ADMIN_KEY` (32 to 512 printable characters). It manages workspaces (create one with its first secret key, list them, mint a new secret key for a workspace that lost its keys, invite a workspace's first owner) and reads the server's Prometheus metrics at `GET /metrics`, and nothing else. A server without it refuses those routes. Platforms with one workspace per customer use it to set each one up.

## Rotating and revoking

To rotate a secret key without downtime: create a second key, move your backend to it, then revoke the first. Both work in between. A key cannot revoke itself (`403`, `reason` `self_revocation`): revoke it with another secret key, or in the console, where an owner or admin can revoke any key. Revoking takes effect at once, also for the session tokens it signed.

## The console session

Staff signed in to the console call the same `/v1` routes as a secret key, with the console's session cookie instead of a key. Their [role](workspaces.md#roles) decides which routes they may call. A program should use a secret key, not a person's session.

## In the API

[API keys](../api/keys.md): create, list, change the origins of, and revoke keys. [Workspaces](../api/workspaces.md): the admin key's routes.
