//! The document's component schemas: every request and response body, by
//! name. The router tests validate what the server sends and accepts against
//! them (`check.rs`): an object lists every field it may have
//! (`additionalProperties: false`) and requires every field it always has.

use serde_json::{Map, Value, json};

use super::super::Code;
use super::schema_ref as r;
use crate::console::Permission;

/// An object with `required` fields always present and `optional` ones
/// sometimes; nothing else.
fn object(required: &[(&str, Value)], optional: &[(&str, Value)]) -> Value {
    let mut properties = Map::new();
    for (name, schema) in required.iter().chain(optional) {
        properties.insert((*name).to_owned(), schema.clone());
    }
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        let names: Vec<&str> = required.iter().map(|(name, _)| *name).collect();
        schema["required"] = json!(names);
    }
    schema
}

fn string() -> Value {
    json!({ "type": "string" })
}

fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn time() -> Value {
    json!({ "type": "string", "format": "date-time" })
}

fn integer() -> Value {
    json!({ "type": "integer" })
}

fn range(min: u64, max: u64) -> Value {
    json!({ "type": "integer", "minimum": min, "maximum": max })
}

fn boolean() -> Value {
    json!({ "type": "boolean" })
}

fn array(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

/// `schema` or `null`.
fn nullable(schema: Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some(t) if schema.get("$ref").is_none() => {
            let mut schema = schema.clone();
            schema["type"] = json!([t, "null"]);
            schema
        }
        _ => json!({ "anyOf": [schema, { "type": "null" }] }),
    }
}

fn described(mut schema: Value, description: &str) -> Value {
    schema["description"] = description.into();
    schema
}

/// A page of `item` (08 §6.1).
fn list(item: &str) -> Value {
    object(
        &[("data", array(r(item))), ("next", nullable(string()))],
        &[],
    )
}

fn key(with_secret: bool) -> Value {
    let mut required = vec![
        ("id", text("`key_` and 32 hex digits; not the key.")),
        ("kind", json!({ "enum": ["secret", "publishable"] })),
        ("name", string()),
        ("prefix", text("The key's first 11 characters.")),
        (
            "allowed_origins",
            described(
                nullable(array(string())),
                "Publishable keys only; `null` accepts every origin.",
            ),
        ),
        ("created_at", time()),
        ("last_used_at", nullable(time())),
        ("revoked_at", nullable(time())),
    ];
    if with_secret {
        required.push(("key", text("The key itself, shown only in this response.")));
    }
    object(&required, &[])
}

fn workspace(created: bool) -> Value {
    let mut required = vec![
        ("id", text("`ws_` and 32 hex digits.")),
        ("key", string()),
        ("name", string()),
        ("created_at", time()),
    ];
    if created {
        required.push(("secret_key", r("KeyCreated")));
    }
    object(&required, &[])
}

fn hold_ttl() -> Value {
    described(range(60, 7_200), "Seconds.")
}

/// Every schema, by name.
pub(super) fn all() -> Map<String, Value> {
    let codes: Vec<&str> = Code::ALL.iter().map(|c| c.as_str()).collect();
    let permissions: Vec<&str> = Permission::ALL.iter().map(|p| p.as_str()).collect();
    let roles = || json!({ "enum": ["owner", "admin", "editor", "operator", "viewer"] });
    let user_id = || text("`usr_` and 32 hex digits.");
    let status_wire = json!({ "oneOf": [
        { "enum": ["free", "blocked"] },
        described(range(4, 15), "A custom status code."),
    ] });
    let object_status = json!({ "oneOf": [
        { "enum": ["free", "held", "booked", "blocked"] },
        described(range(4, 15), "A custom status code."),
    ] });
    let entries = [
        (
            "Problem",
            described(
                object(
                    &[
                        ("code", json!({ "enum": codes })),
                        ("status", integer()),
                        ("message", text("For humans; may change. Match on `code`.")),
                        (
                            "details",
                            described(
                                json!({ "type": "object" }),
                                "Empty, or what the code adds (08 §6.4): `reason`, `missing`, \
`objects`, `field`, `limit`, `requested`, `session`, `findings`, `draft_etag`, `version`.",
                            ),
                        ),
                        ("request_id", text("Equals `Cavea-Request-Id`.")),
                    ],
                    &[],
                ),
                "An error (08 §6.4).",
            ),
        ),
        (
            "CreateWorkspace",
            object(&[("key", string()), ("name", string())], &[]),
        ),
        ("Workspace", workspace(false)),
        ("WorkspaceCreated", workspace(true)),
        ("WorkspaceList", list("Workspace")),
        ("MintKey", object(&[("name", string())], &[])),
        (
            "CreateKey",
            object(
                &[
                    ("kind", json!({ "enum": ["secret", "publishable"] })),
                    ("name", string()),
                ],
                &[(
                    "allowed_origins",
                    described(
                        nullable(
                            json!({ "type": "array", "items": string(), "minItems": 1, "maxItems": 50 }),
                        ),
                        "Publishable keys only: origins such as `https://tickets.example.com`.",
                    ),
                )],
            ),
        ),
        (
            "UpdateKey",
            object(
                &[(
                    "allowed_origins",
                    described(
                        nullable(
                            json!({ "type": "array", "items": string(), "minItems": 1, "maxItems": 50 }),
                        ),
                        "Origins such as `https://tickets.example.com`; `null` allows every origin.",
                    ),
                )],
                &[],
            ),
        ),
        ("Key", key(false)),
        ("KeyCreated", key(true)),
        ("KeyList", list("Key")),
        (
            "ChartDocument",
            json!({
                "type": "object",
                "description": "A chart document (`docs/03-chart-format.md` §2), stored as sent.",
            }),
        ),
        (
            "CreateChart",
            object(
                &[("key", string()), ("name", string())],
                &[("document", nullable(r("ChartDocument")))],
            ),
        ),
        (
            "Stats",
            object(
                &[
                    ("seats", integer()),
                    ("rows", integer()),
                    ("segments", integer()),
                    ("sections", integer()),
                    ("floors", integer()),
                    ("public_bytes", integer()),
                    ("public_br_bytes", nullable(integer())),
                    ("full_bytes", integer()),
                ],
                &[],
            ),
        ),
        (
            "Version",
            object(
                &[
                    ("id", text("`cv_` and 32 hex digits.")),
                    ("number", integer()),
                    (
                        "hash",
                        text("The public artefact's content hash: 32 hex digits."),
                    ),
                    (
                        "url",
                        text("Where the compiled chart is served, relative to the server."),
                    ),
                    ("stats", r("Stats")),
                    ("created_at", time()),
                    (
                        "thumbnail",
                        described(
                            nullable(object(
                                &[
                                    ("hash", text("The asset's hash: 32 hex digits.")),
                                    ("url", text("`/v1/public/assets/{hash}`.")),
                                ],
                                &[],
                            )),
                            "An image the publisher attached, or `null`.",
                        ),
                    ),
                    (
                        "created_by",
                        described(
                            nullable(string()),
                            "Who published it: `usr_…` or `key_…`; `null` if not recorded \
(published with a key before this was).",
                        ),
                    ),
                ],
                &[(
                    "created_by_email",
                    described(
                        nullable(string()),
                        "In the versions list only: the publisher's email, for a console \
member whose role may see members, while the publisher is one; otherwise `null`.",
                    ),
                )],
            ),
        ),
        ("VersionList", list("Version")),
        (
            "SetThumbnail",
            object(
                &[(
                    "asset",
                    text("An uploaded image's hash (`POST /v1/assets`): 32 hex digits."),
                )],
                &[],
            ),
        ),
        (
            "LockChart",
            object(
                &[],
                &[(
                    "take_over",
                    described(
                        boolean(),
                        "Take the lock from another who holds a live one. Default false.",
                    ),
                )],
            ),
        ),
        (
            "ChartLock",
            object(
                &[
                    ("holder", text("`usr_…`, or `key_…` for an API key.")),
                    (
                        "email",
                        described(
                            nullable(string()),
                            "The holder's email, for a console member who may edit charts; \
otherwise `null`.",
                        ),
                    ),
                    ("since", time()),
                    (
                        "expires_at",
                        described(time(), "The lock ends then unless renewed."),
                    ),
                ],
                &[],
            ),
        ),
        (
            "ChartLockState",
            object(&[("lock", nullable(r("ChartLock")))], &[]),
        ),
        (
            "Chart",
            object(
                &[
                    ("key", string()),
                    ("name", string()),
                    ("draft_etag", text("Pass back verbatim in `If-Match`.")),
                    ("draft_updated_at", time()),
                    ("published_version", nullable(r("Version"))),
                    ("archived_at", nullable(time())),
                    ("created_at", time()),
                ],
                &[],
            ),
        ),
        ("ChartList", list("Chart")),
        (
            "Finding",
            object(
                &[
                    ("code", text("An E or W code (03 §3.2), or `unsupported`.")),
                    ("severity", json!({ "enum": ["error", "warning"] })),
                    ("node", nullable(integer())),
                    ("related", nullable(integer())),
                    ("message", string()),
                ],
                &[],
            ),
        ),
        (
            "Validation",
            object(
                &[
                    ("draft_etag", string()),
                    ("valid", boolean()),
                    ("findings", array(r("Finding"))),
                    ("stats", nullable(r("Stats"))),
                ],
                &[],
            ),
        ),
        (
            "EventSettings",
            object(
                &[
                    ("hold_ttl_seconds", hold_ttl()),
                    ("max_objects", range(1, 100)),
                    ("require_session", boolean()),
                    ("max_tokens_per_sub", range(1, 10)),
                ],
                &[],
            ),
        ),
        (
            "EventSettingsInput",
            object(
                &[],
                &[
                    ("hold_ttl_seconds", nullable(hold_ttl())),
                    ("max_objects", nullable(range(1, 100))),
                    ("require_session", nullable(boolean())),
                    ("max_tokens_per_sub", nullable(range(1, 10))),
                ],
            ),
        ),
        (
            "Counts",
            object(
                &[
                    ("free", integer()),
                    ("held", integer()),
                    ("booked", integer()),
                    ("blocked", integer()),
                    (
                        "custom",
                        described(
                            json!({ "type": "object", "additionalProperties": integer() }),
                            "Objects per custom status code.",
                        ),
                    ),
                ],
                &[],
            ),
        ),
        (
            "Event",
            object(
                &[
                    ("key", string()),
                    ("name", nullable(string())),
                    ("chart", text("The chart's key.")),
                    ("version", r("Version")),
                    ("settings", r("EventSettings")),
                    ("archived_at", nullable(time())),
                    ("created_at", time()),
                    (
                        "seq",
                        described(
                            integer(),
                            "The committed `seq`: it changes exactly when the seat state does.",
                        ),
                    ),
                    ("counts", r("Counts")),
                ],
                &[],
            ),
        ),
        ("EventList", list("Event")),
        (
            "CreateEvent",
            object(
                &[("key", string()), ("chart", text("The chart's key."))],
                &[
                    (
                        "version",
                        nullable(described(
                            integer(),
                            "A version number; the published one by default.",
                        )),
                    ),
                    ("name", nullable(string())),
                    ("settings", r("EventSettingsInput")),
                ],
            ),
        ),
        (
            "UpdateEvent",
            object(
                &[],
                &[
                    ("name", nullable(string())),
                    ("settings", nullable(r("EventSettingsInput"))),
                ],
            ),
        ),
        (
            "ObjectRefs",
            json!({
                "description": "Labels, or `idx` in the event's chart version (08 §6.1). At most 1,000.",
                "oneOf": [
                    { "type": "array", "items": string(), "maxItems": 1_000 },
                    object(
                        &[
                            ("version", text("The public hash of the event's chart version.")),
                            ("idx", json!({ "type": "array", "items": { "type": "integer", "minimum": 0 }, "maxItems": 1_000 })),
                        ],
                        &[],
                    ),
                ],
            }),
        ),
        (
            "ObjectLabel",
            object(&[("label", string()), ("oid", integer())], &[]),
        ),
        (
            "CreateHoldToken",
            object(
                &[("event", text("The event's key."))],
                &[
                    ("ttl_seconds", nullable(hold_ttl())),
                    (
                        "max_objects",
                        nullable(described(range(1, 1_000_000), "Unlimited by default.")),
                    ),
                ],
            ),
        ),
        (
            "PublicCreateHoldToken",
            object(
                &[("event", text("The event's key."))],
                &[(
                    "max_objects",
                    nullable(described(
                        range(1, 100),
                        "The event's by default, and at most it.",
                    )),
                )],
            ),
        ),
        (
            "HoldTokenCreated",
            object(
                &[
                    (
                        "hold_token",
                        text("`ht_` and 64 hex digits, shown only here."),
                    ),
                    ("event", string()),
                    ("seq", integer()),
                    ("expires_at", time()),
                    ("max_objects", nullable(integer())),
                ],
                &[],
            ),
        ),
        (
            "HoldTokenState",
            object(
                &[
                    ("event", string()),
                    ("expires_at", time()),
                    ("max_objects", nullable(integer())),
                    ("objects", array(r("ObjectLabel"))),
                ],
                &[],
            ),
        ),
        (
            "ExtendHoldToken",
            object(&[], &[("ttl_seconds", nullable(hold_ttl()))]),
        ),
        (
            "ExtendReply",
            object(&[("seq", integer()), ("expires_at", time())], &[]),
        ),
        (
            "Hold",
            object(
                &[("hold_token", string()), ("objects", r("ObjectRefs"))],
                &[],
            ),
        ),
        (
            "HoldReply",
            object(
                &[
                    ("seq", integer()),
                    ("expires_at", time()),
                    ("objects", array(r("ObjectLabel"))),
                ],
                &[],
            ),
        ),
        (
            "Release",
            described(
                object(
                    &[],
                    &[
                        ("hold_token", nullable(string())),
                        ("objects", nullable(r("ObjectRefs"))),
                    ],
                ),
                "With `hold_token`: held objects, all of the token's if `objects` is left out. \
Without: booked objects.",
            ),
        ),
        (
            "PublicRelease",
            object(
                &[("hold_token", string())],
                &[("objects", nullable(r("ObjectRefs")))],
            ),
        ),
        (
            "Book",
            object(
                &[("order_ref", string())],
                &[
                    ("hold_token", nullable(string())),
                    ("objects", nullable(r("ObjectRefs"))),
                    (
                        "ticket_types",
                        described(
                            json!({ "type": "object", "additionalProperties": string() }),
                            "A ticket type per label.",
                        ),
                    ),
                    (
                        "extra",
                        described(
                            json!({ "type": "object", "additionalProperties": { "type": "object" } }),
                            "A JSON object per label, stored as sent.",
                        ),
                    ),
                ],
            ),
        ),
        (
            "ChangeStatus",
            object(
                &[("objects", r("ObjectRefs")), ("status", status_wire)],
                &[],
            ),
        ),
        (
            "SeatReply",
            object(
                &[("seq", integer()), ("objects", array(r("ObjectLabel")))],
                &[],
            ),
        ),
        (
            "EventObject",
            object(
                &[
                    ("label", string()),
                    ("oid", integer()),
                    ("status", object_status),
                ],
                &[
                    ("seq", integer()),
                    ("expires_at", time()),
                    ("order_ref", string()),
                    ("ticket_type", nullable(string())),
                    ("extra", nullable(json!({ "type": "object" }))),
                ],
            ),
        ),
        (
            "Objects",
            json!({ "oneOf": [
                object(
                    &[("data", array(r("EventObject"))), ("unknown", array(string()))],
                    &[],
                ),
                object(
                    &[("data", array(r("EventObject"))), ("next", nullable(string()))],
                    &[],
                ),
            ] }),
        ),
        (
            "PublicEvent",
            object(
                &[
                    ("event", string()),
                    (
                        "chart",
                        object(&[("hash", string()), ("url", string())], &[]),
                    ),
                    ("hold_ttl_seconds", integer()),
                    ("max_objects", integer()),
                    ("require_session", boolean()),
                    ("licence", r("PublicLicence")),
                ],
                &[],
            ),
        ),
        (
            "PublicLicence",
            described(
                object(
                    &[
                        (
                            "status",
                            json!({ "enum": ["licensed", "expired", "unlicensed"] }),
                        ),
                        (
                            "domains",
                            described(
                                array(string()),
                                "While `licensed`: the host names the renderer runs on \
unmarked, lowercase; `*.example.com` is every name under `example.com`, not the name itself. \
Empty otherwise.",
                            ),
                        ),
                    ],
                    &[],
                ),
                "What the renderer needs of the server's licence to decide its mark (12 §7.4): \
`licensed` (valid or in grace), `expired`, or `unlicensed` (missing or invalid).",
            ),
        ),
        (
            "LicenceUpload",
            object(
                &[(
                    "licence",
                    text("The licence file's text, exactly as issued (at most 16 KiB)."),
                )],
                &[],
            ),
        ),
        (
            "ConsoleLicence",
            object(
                &[
                    (
                        "state",
                        json!({ "enum": ["valid", "grace", "expired", "missing", "invalid"] }),
                    ),
                    (
                        "read_only",
                        described(
                            boolean(),
                            "`expired`: the server refuses console writes to charts and \
events (`403` `licence_expired`); seat operations, keys, members and settings go on.",
                        ),
                    ),
                    (
                        "grace_until",
                        described(
                            nullable(json!({ "type": "string", "format": "date" })),
                            "While `grace`: the last day of grace (UTC).",
                        ),
                    ),
                    (
                        "manage",
                        described(
                            boolean(),
                            "Whether this user owns every workspace on the server: may upload \
a licence and sees the fields below, which are otherwise `null`.",
                        ),
                    ),
                    ("source", nullable(json!({ "enum": ["setting", "upload"] }))),
                    (
                        "reason",
                        described(
                            nullable(string()),
                            "While `invalid`: why (`unreadable`, `too_large`, `not_utf8`, \
`malformed`, `version`, `unknown_key`, `signature`, `payload`, `dates`).",
                        ),
                    ),
                    (
                        "licence",
                        nullable(object(
                            &[
                                ("id", string()),
                                ("customer", string()),
                                ("plan", string()),
                                ("features", array(string())),
                                ("domains", array(string())),
                                ("max_workspaces", nullable(integer())),
                                ("issued_at", json!({ "type": "string", "format": "date" })),
                                (
                                    "expires_at",
                                    described(
                                        json!({ "type": "string", "format": "date" }),
                                        "Valid through this day (UTC); then `grace_days` \
days of grace.",
                                    ),
                                ),
                                ("grace_days", integer()),
                                ("key_id", string()),
                            ],
                            &[],
                        )),
                    ),
                ],
                &[],
            ),
        ),
        (
            "ConsoleSetup",
            object(
                &[
                    (
                        "token",
                        text("The one-time token of the setup link's fragment."),
                    ),
                    (
                        "email",
                        text("The owner's email; stored trimmed and lowercased."),
                    ),
                    (
                        "password",
                        described(
                            json!({ "type": "string", "minLength": 15, "maxLength": 128 }),
                            "15 to 128 characters; no other rule.",
                        ),
                    ),
                ],
                &[(
                    "workspace",
                    nullable(described(
                        object(&[("key", string()), ("name", string())], &[]),
                        "The first workspace, created only when none exists (default key \
`main`, name `Main`). Existing workspaces all get the owner.",
                    )),
                )],
            ),
        ),
        (
            "ConsoleSignIn",
            object(&[("email", string()), ("password", string())], &[]),
        ),
        (
            "ConsolePassword",
            object(
                &[
                    ("current_password", string()),
                    (
                        "new_password",
                        described(
                            json!({ "type": "string", "minLength": 15, "maxLength": 128 }),
                            "15 to 128 characters; no other rule.",
                        ),
                    ),
                ],
                &[],
            ),
        ),
        (
            "ConsoleMe",
            object(
                &[
                    (
                        "user",
                        object(
                            &[
                                (
                                    "id",
                                    text("`usr_` and 32 hex digits: the actor in event logs."),
                                ),
                                ("email", string()),
                            ],
                            &[],
                        ),
                    ),
                    (
                        "workspaces",
                        array(object(
                            &[
                                (
                                    "id",
                                    text("`ws_` and 32 hex digits: send it in `Cavea-Workspace`."),
                                ),
                                ("key", string()),
                                ("name", string()),
                                ("role", roles()),
                                (
                                    "permissions",
                                    described(
                                        array(json!({ "enum": permissions })),
                                        "What the role allows: compare with an operation's \
`x-cavea-permission`.",
                                    ),
                                ),
                            ],
                            &[],
                        )),
                    ),
                    (
                        "csrf_token",
                        text("Send it in `Cavea-Csrf-Token` with every non-safe request."),
                    ),
                    (
                        "session",
                        object(
                            &[
                                ("expires_at", described(time(), "Unless used again.")),
                                (
                                    "absolute_expires_at",
                                    described(time(), "However it is used."),
                                ),
                            ],
                            &[],
                        ),
                    ),
                    ("licence", r("ConsoleLicence")),
                ],
                &[],
            ),
        ),
        (
            "WorkspaceMember",
            object(
                &[
                    (
                        "user",
                        object(&[("id", user_id()), ("email", string())], &[]),
                    ),
                    ("role", roles()),
                ],
                &[],
            ),
        ),
        ("MemberList", list("WorkspaceMember")),
        ("ChangeRole", object(&[("role", roles())], &[])),
        (
            "InviteMember",
            object(&[("email", string()), ("role", roles())], &[]),
        ),
        ("InviteOwner", object(&[("email", string())], &[])),
        (
            "Invitation",
            object(
                &[
                    ("id", text("`inv_` and 32 hex digits.")),
                    ("email", string()),
                    ("role", roles()),
                    ("created_at", time()),
                    ("expires_at", time()),
                ],
                &[],
            ),
        ),
        ("InvitationList", list("Invitation")),
        (
            "InvitationCreated",
            object(
                &[
                    ("invitation", r("Invitation")),
                    (
                        "link",
                        text(
                            "`{CAVEA_PUBLIC_URL}/console/invite#token=<64 hex digits>`, shown \
only here: give it to the person invited. The console's page posts the token to \
`/console/api/invitations/accept`.",
                        ),
                    ),
                ],
                &[],
            ),
        ),
        (
            "AcceptInvitation",
            object(
                &[("token", text("The 64 hex digits of the link's fragment."))],
                &[(
                    "password",
                    nullable(described(
                        json!({ "type": "string", "minLength": 15, "maxLength": 128 }),
                        "Not signed in, for an email with no account: the new user's password.",
                    )),
                )],
            ),
        ),
        ("RenameWorkspace", object(&[("name", string())], &[])),
        (
            "ConsoleWorkspace",
            object(
                &[
                    ("id", text("`ws_` and 32 hex digits.")),
                    ("key", string()),
                    ("name", string()),
                ],
                &[],
            ),
        ),
        (
            "AuditEntry",
            object(
                &[
                    ("id", text("`aud_` and a number.")),
                    ("at", time()),
                    (
                        "actor",
                        text("`usr_…`, `key_…`, `admin`, or `anonymous` (a failed sign-in)."),
                    ),
                    ("action", json!({ "enum": crate::audit::ACTIONS })),
                    (
                        "target",
                        described(
                            nullable(string()),
                            "What was acted on: `usr_…`, `key_…`, `inv_…`, `ws_…`, or a chart's \
or event's key.",
                        ),
                    ),
                    (
                        "detail",
                        described(
                            nullable(json!({ "type": "object" })),
                            "Small facts: `role`, `from` and `to`, `kind`, `version`.",
                        ),
                    ),
                    ("request_id", nullable(string())),
                ],
                &[],
            ),
        ),
        ("AuditList", list("AuditEntry")),
        (
            "Asset",
            object(
                &[
                    ("hash", text("The content hash: 32 lowercase hex digits.")),
                    (
                        "content_type",
                        json!({ "enum": ["image/png", "image/jpeg", "image/webp"] }),
                    ),
                    ("size", described(integer(), "Bytes.")),
                    ("width", integer()),
                    ("height", integer()),
                    ("url", text("Where it is served, relative to the server.")),
                ],
                &[],
            ),
        ),
    ];
    entries
        .into_iter()
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect()
}
