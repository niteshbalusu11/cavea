# Error codes

Every error the Cavea server returns, what it means and what to do about it. At the end of this page you can handle each error by its `code`, and by its `reason` where it has one.

## The shape of an error

An error is a JSON body with the content type `application/problem+json`, always with these five members:

```json
{
  "code": "object_not_free",
  "status": 409,
  "message": "At least one object is not free.",
  "details": { "objects": [{ "label": "-E-10", "oid": 57, "reason": "not_free" }] },
  "request_id": "req_6ad9b530f0e172e7214232f15266b459"
}
```

| Member | What it is |
|---|---|
| `code` | What went wrong, from the list below. It never changes meaning: match on it. |
| `status` | The HTTP status, always the one listed with the code. |
| `message` | A sentence for people. It may change between versions: never match on it. |
| `details` | An object, empty when there is nothing to add. Many codes put a `reason` here, and some add the objects, field or limit at fault. |
| `request_id` | The same value as the `Cavea-Request-Id` response header. Quote it when you report a problem: the server logs every request under it. |

A seat command is all or nothing: when the server refuses it (a `4xx` status), nothing changed. After a `500` or `503` the outcome of a seat command is not known: the change may have been stored while the reply was lost. Send seat commands from your backend with an `Idempotency-Key` and retry with the same key, which returns the first outcome, or read the seats before trying again.

Retry only where this page says to. `429` and `503` carry a `Retry-After` header with the seconds to wait when the server knows them.

<h2 id="status-400">400 Bad request</h2>

### `invalid_request`

The request is malformed: a body, header, path or query parameter the server cannot take. Fix the request; sending it again unchanged fails again. `details.reason` says what is wrong, and some reasons add `field` or `limit`.

| Reason | Meaning | What to do |
|---|---|---|
| `json_syntax` | The body is not valid JSON. | Send valid JSON. |
| `json_data` | The JSON does not have the expected shape: a missing or unknown field, or a value of the wrong type. `message` names the problem. | Compare the body with the operation's schema in the [API reference](../api/index.md). |
| `content_type` | The body was sent without `Content-Type: application/json`. | Send the header. |
| `body_too_large` | The body is over the route's size limit; `limit` is the limit in bytes when the route says it. | Send less: see [Limits](limits.md#requests). |
| `body_unreadable` | The body could not be read, for example because the connection broke. | Send it again. |
| `json_depth` | Arrays and objects nest deeper than `limit` (32). | Flatten the body. |
| `invalid_field` | A field's value is not allowed, such as a name over 128 characters or an event setting out of its range; `field` names it (`settings.hold_ttl_seconds`), and `message` says why. | Fix that field. |
| `path` | A path parameter is malformed, such as a version number that is not a number. | Fix the URL. |
| `query` | A query parameter or a list cursor is malformed, or `limit` is outside 1 to 100. | Fix the query; pass `next` back as `after` unchanged. |
| `too_many_objects` | The request names more objects than one command takes (`limit`, 1,000). | Split it. |
| `no_objects` | A hold, release, book or status change names no object. | Name at least one. |
| `ttl_out_of_range` | The `ttl_seconds` of a hold token your backend creates or extends is outside 60 to 7,200 seconds. (An event's hold lifetime out of range is `invalid_field`.) | Choose one in range. |
| `zero_object_limit` | A hold token's object limit is 0. | Give it 1 or more. |
| `token_already_exists` | The new hold token the server generated is already in use. With 256 random bits this does not happen in practice. | Create the token again. |
| `order_ref_length` | `order_ref` is empty, longer than 128 characters, or contains U+0000. | Send 1 to 128 characters. |
| `ticket_type_length` | A `ticket_type` is empty, longer than 128 characters, or contains U+0000. | Send 1 to 128 characters. |
| `extra_too_large` | A booking's `extra` for one object is over 2,048 bytes. | Keep the extra data small, or keep it in your own system. |
| `detail_mismatch` | A booking's per-object details name an object that is not being booked, or one object twice. | Name each booked object at most once. |
| `bad_status` | A status change asks for a status other than free, blocked or a custom status. | Hold and book with their own operations. |
| `idempotency_key` | The `Idempotency-Key` header is not 1 to 255 printable ASCII characters. | Use a shorter key of plain characters, such as a UUID. |
| `idempotency_key_length` | The same, found by the seat engine. | As above. |
| `if_match` | Saving a draft without `If-Match`, or with one that is not a draft's `draft_etag` or `*`. | Send the `draft_etag` you last read, or `*` to overwrite. |
| `document` | The draft is not a chart document the server can store: not a JSON object, a schema version it does not know, or text with U+0000. | Send a document the designer exported, or fix it. |
| `asset_type` | An uploaded image is not PNG, JPEG or WebP. | Convert it to one of those. |
| `asset_malformed` | An uploaded image is truncated, has data after its end, or has a header that does not parse. | Export the image again. |
| `asset_dimensions` | An uploaded image has a side of 0 or over `limit` pixels (8,192). | Scale it down. |
| `archived` | The chart or event is archived: a chart's draft can no longer change and a chart takes no new events; an event takes no seat operations. | Use another chart or event. |
| `unpublished` | An event was created on a chart with no published version. | [Publish](../api/charts.md#publishChart) the chart first. |
| `revoked` | Changing the allowed origins of a revoked key. | Create a new key. |
| `workspace` | A request with a console session did not name its workspace in `Cavea-Workspace`. | Send the workspace's id (`ws_…`) from `GET /console/api/me`. |
| `last_owner` | The change would leave the workspace without an owner. | Make another member an owner first. |
| `upgrade` | The realtime route was called without a WebSocket upgrade. | Connect with a WebSocket client. |
| `last_event_id` | The server-sent events stream got a `Last-Event-ID` that is not a sequence number. | Let the browser's `EventSource` send it. |
| `no_second_factor` | Confirming or turning off two-step sign-in for an account that has none to confirm or turn off. | Start again from the account page. |

<h2 id="status-401">401 Unauthorized</h2>

### `unauthorized`

The credential is missing, malformed or not valid. Check which credential the operation takes in the [API reference](../api/index.md), and see [Keys](../concepts/keys.md).

| Reason | Meaning | What to do |
|---|---|---|
| `missing` | No `Authorization` header, or no console session cookie on a console route. | Send `Authorization: Bearer <key>`, or sign in. |
| `malformed` | `Authorization` is not `Bearer <token>`. | Fix the header. |
| `invalid` | The key is unknown or revoked, or the admin key is wrong. | Use a live key of this server. |
| `session_token` | The event requires a [session token](../concepts/session-tokens.md), or one was sent and failed. `details.session` says why: `missing`, `invalid` (structure, signature, signing key or claims), `expired`, or `event` (signed for another event). | Have your backend sign a fresh token for this buyer and event. |
| `console_session` | The console session has ended or is unknown. | Sign in to the console again. |
| `credentials` | Signing in to the console with a wrong email or password. | Check them. Repeated failures slow further attempts (`429`, `backoff`). |
| `setup_token` | The setup link is wrong, expired or used, or the server already has its owner. | Restart the server for a new link while it has no user; otherwise sign in. |
| `invitation` | The invitation link is wrong, expired or used. | Ask for a new invitation. |
| `challenge` | The second step of a sign-in came too late (after five minutes), after five wrong codes, or twice. | Sign in again with the email and password. |
| `second_factor` | The code from the authenticator app, or the recovery code, is wrong or was used already. Repeated failures slow further attempts (`429`, `backoff`). | Type the current code, or an unused recovery code. |
| `reset_token` | The password reset link is wrong, expired (after an hour) or used. | Ask an owner or admin for a new link. |

<h2 id="status-403">403 Forbidden</h2>

### `forbidden`

The credential is valid, but may not do this. Without a `reason`, the seat engine refused a command the key's kind may not issue, such as booking with a publishable key.

| Reason | Meaning | What to do |
|---|---|---|
| `key_kind` | The wrong kind of credential for the route: a secret key on a buyer route, a publishable key on a backend route, an API key on an admin route. | Use the kind the operation takes. |
| `origin` | The publishable key does not allow the page's origin, or a console request did not come from the console's own origin. | Add the origin to the key's allowed origins, or call from the right page. |
| `session` | The hold token was created with a session token for another buyer (`sub`), or is used without a session token. | Send the session token of the buyer who created the hold token. |
| `self_revocation` | A key tried to revoke itself. | Revoke it with another secret key, or in the console. |
| `csrf` | A console request that changes something lacks the session's CSRF token. | Send `Cavea-Csrf-Token` with the session's `csrf_token`. |
| `workspace` | The console session's person is not a member of the workspace named in `Cavea-Workspace`. | Name a workspace you belong to. |
| `role` | The member's role does not grant the operation's permission; `details.permission` names it. | Ask an owner or admin for a role that does: see [Workspaces and roles](../concepts/workspaces.md). |
| `licence_expired` | The licence has expired, so the console refuses changes to charts and events. Seat operations still work. | Upload a renewed licence. |
| `instance_owner` | Only a person who owns every workspace on the server may change its licence. | Ask that person. |
| `credentials` | Changing the console password with a wrong current password. | Enter the current password. |
| `invitation_email` | The invitation is for another email address. | Sign in as the invited address. |
| `second_factor` | Turning on, or off, two-step sign-in with a wrong code. | Type the current code from the authenticator app (or, to turn it off, an unused recovery code). |
| `own_account` | Making a reset link for, or resetting the two-step sign-in of, your own account from the members list. | Change your password, or turn off two-step sign-in, from your account page. |
| `other_workspaces` | Resetting someone who also belongs to a workspace where you are not an owner or admin (or, for an owner there, not an owner). An account is the whole server's, not one workspace's. | Ask an owner of that workspace to do it. |

<h2 id="status-404">404 Not found</h2>

### `not_found`

Nothing is at this path for this credential: no such chart, event, key, version or workspace, or one that belongs to another workspace, or no route at all. Seat commands say what is missing in `details.missing`: `hold_token` (unknown, or expired and therefore gone), or `objects`, with `details.objects` listing each reference that matches no object. Check the keys and labels; an expired hold token cannot come back, so start a new one.

<h2 id="status-409">409 Conflict</h2>

### `already_exists`

Something with this key already exists in the workspace, possibly archived: a chart, an event, a workspace, or a member with this email. Choose another key, or use the existing one.

| Reason | Meaning | What to do |
|---|---|---|
| `sign_in` | Accepting an invitation as a new account, for an email that already has one. | Sign in, then open the invitation link again. |
| `second_factor` | Starting two-step sign-in for an account that already has it on. | Turn it off first to start again with a new authenticator. |

### `object_not_free`

A hold or a status change named an object that is not free. `details.objects` lists each one as `label`, `oid` and `reason`. Nothing was held: show the buyer the seat is gone and let them choose again.

| Reason | Meaning | What to do |
|---|---|---|
| `not_free` | The object is held, booked or blocked. | Choose another object. |

### `object_not_held_by_token`

A book or release with a hold token named an object the token does not hold. The hold may have expired, or the object was never held by it. `details.objects` lists them.

| Reason | Meaning | What to do |
|---|---|---|
| `not_held_by_token` | The object is not held by this hold token. | Hold it again, or book only what the token holds. |

### `object_status_conflict`

The object's status does not allow the change. `details.objects` lists them.

| Reason | Meaning | What to do |
|---|---|---|
| `not_booked` | A release without a hold token, of an object that is not booked. Held objects are released with their token, or expire. | Release with the hold token, or let it expire. |
| `not_blocked` | A change to free, of an object that is not blocked or in a custom status. | Leave it, or release a booking instead. |

### `version_conflict`

The chart's draft has changed since you read it: `details.draft_etag` is the current one. Or objects were named by index in a chart version that is not the event's: `details.version` is the event's. Read the draft again and reapply your change, or address objects by label.

### `chart_locked`

Someone else holds the chart's editing lock: `details.lock` says who and until when. Wait for them to finish, or take the lock over if you may.

### `upgrade_conflict`

Reserved for moving an event to a newer chart version, which the server does not do yet. Not sent today.

### `idempotency_mismatch`

The `Idempotency-Key` was used before, within 24 hours, with a different request. Use a new key for a new request; resend the same request to get the first reply again.

<h2 id="status-410">410 Gone</h2>

### `hold_token_expired`

Reserved. The server does not send it: an expired hold token is deleted when it expires, so a request with it is `404 not_found` with `details.missing` `hold_token`.

<h2 id="status-422">422 Unprocessable</h2>

### `hold_limit_exceeded`

The hold would put more objects under the hold token than its limit: `details.limit` is the limit and `details.requested` what the token would hold. Hold fewer, or release some first. The limit for a buyer's page is the event's **Seats per hold token**, 10 unless changed.

| Reason | Meaning | What to do |
|---|---|---|
| `tokens_per_sub` | Creating a hold token for a buyer (`sub` of the session token) who already has the event's most live hold tokens (`limit`, 2 unless changed). | Reuse the buyer's current hold token, or wait for one to expire. |

### `chart_invalid`

Publishing failed because the draft has validation errors: `details.findings` lists them, each with a code, a message and where it is. Fix them in the designer (its Validation panel shows the same list) and publish again.

### `channel_not_allowed`

Reserved for channels, which the server does not have yet. Not sent today.

### `orphan_rule_violated`

Reserved for the rule against leaving single seats, which the server does not enforce yet. Not sent today.

### `not_enough_objects`

Reserved for best-available, which the server does not have yet. Not sent today.

<h2 id="status-429">429 Too many requests</h2>

### `rate_limited`

Too many requests. Wait the seconds in `Retry-After`, then retry. Without a `reason`, a buyer's client went over a [rate limit](limits.md#rate-limits) of its publishable key from its address, or over the realtime connections one address may hold. If many buyers share one address because the server is behind a proxy or CDN, the operator lists that proxy in `CAVEA_TRUSTED_PROXIES`.

| Reason | Meaning | What to do |
|---|---|---|
| `sign_in` | Too many console sign-in attempts from this address in a minute. | Wait and try again. |
| `backoff` | Too many failed sign-ins for this account; the server waits longer after each. | Wait the `Retry-After` seconds. |

<h2 id="status-500">500 Internal server error</h2>

### `internal_error`

Something unexpected went wrong in the server. The cause is in the server's log under the request id, never in the reply. A seat command that returns it may or may not have been applied: read the state, or retry with the same `Idempotency-Key`. Report it with the `request_id`.

<h2 id="status-503">503 Service unavailable</h2>

### `overloaded`

The server is busy: the event's queue of commands is full, or a chart could not be compiled in time. Retry after `Retry-After` (1 second for a busy event).

| Reason | Meaning | What to do |
|---|---|---|
| `compile_timeout` | Publishing or validating a chart did not finish in time. | Retry shortly. |
| `password_hashing` | Too many console sign-ins are being checked at once. | Retry shortly. |
| `asset_memory` | Too many images are being uploaded or served at once. | Retry shortly. |

### `unavailable`

The server cannot reach PostgreSQL, has run out of database connections, or lost the database's reply to a commit. For a seat command the outcome is not known: the change may have been stored. Retry with the same `Idempotency-Key`, which returns the first outcome if there was one, or read the seats first; back off between retries. If it lasts, the operator checks the database.
