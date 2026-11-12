// The server API reference, generated at build time from the committed
// OpenAPI document (`crates/cavea-server/openapi.json`, itself generated from
// the server's route table and checked against every response in the
// server's tests), so the reference cannot drift from the server. Nothing
// here is committed: the pages exist only in the built site.
//
// Operations are grouped by what a reader looks for (the buyer's browser,
// holds and bookings, events, charts, …) rather than by the document's tags;
// an operation that no group claims fails the build, so a new route is never
// left out.

type Json = Record<string, unknown>;

interface Operation {
  method: string;
  path: string;
  op: Json;
}

interface Group {
  file: string;
  title: string;
  intro: string;
  takes: (o: Operation) => boolean;
}

const SEAT_OPS = new Set(["hold", "release", "book", "changeStatus"]);
const tag = (o: Operation): string => (o.op.tags as string[] | undefined)?.[0] ?? "";

export const GROUPS: Group[] = [
  {
    file: "api/buyers.md",
    title: "Buyer's browser",
    intro:
      "What a buyer's page calls with a [publishable key](../concepts/keys.md): the event and its seat state, hold tokens, holding and releasing seats, the realtime stream, and the compiled chart and images. `caveaServer` in `{{renderer_package}}` makes these calls for you; call them yourself only for a renderer of your own. Every route here answers other origins (CORS).",
    takes: (o) => tag(o) === "public",
  },
  {
    file: "api/seats.md",
    title: "Holds and bookings",
    intro:
      "What your backend calls with a [secret key](../concepts/keys.md): book what a buyer holds, hold and release seats yourself, block seats, and manage [hold tokens](../concepts/holds.md). The same routes take a [console session](console.md).",
    takes: (o) => tag(o) === "hold-tokens" || SEAT_OPS.has(String(o.op.operationId)),
  },
  {
    file: "api/events.md",
    title: "Events",
    intro:
      "Create, read, change and archive [events](../concepts/events.md), and read their seats and seat state.",
    takes: (o) => tag(o) === "events",
  },
  {
    file: "api/charts.md",
    title: "Charts",
    intro:
      "Create [charts](../concepts/charts.md), save and validate drafts, publish versions, take the editing lock, and upload background images.",
    takes: (o) => tag(o) === "charts" || tag(o) === "assets",
  },
  {
    file: "api/keys.md",
    title: "API keys",
    intro: "Create, list, restrict and revoke a workspace's [API keys](../concepts/keys.md).",
    takes: (o) => tag(o) === "keys",
  },
  {
    file: "api/workspaces.md",
    title: "Workspaces",
    intro:
      "What the operator of a server calls with the [admin key](../concepts/keys.md#the-admin-key): create and list [workspaces](../concepts/workspaces.md), mint a secret key for one, invite its first owner.",
    takes: (o) => tag(o) === "workspaces",
  },
  {
    file: "api/console.md",
    title: "Console sessions and members",
    intro:
      "The routes the console itself uses: setup, sign-in and sign-out, the signed-in person, members, invitations, the audit log and the licence. They take the console's session cookie, never an API key. A non-safe request (anything but `GET`) also sends `Cavea-Csrf-Token`, the session's `csrf_token`, and must come from the server's own origin.",
    takes: (o) => tag(o) === "console",
  },
  {
    file: "api/system.md",
    title: "Health, metrics and this document",
    intro:
      "Probes for a load balancer or orchestrator, Prometheus metrics, and the OpenAPI document this reference is built from.",
    takes: (o) => tag(o) === "system",
  },
];

/** Internal specification references (`08 §6.4`, `D-31`, `docs/…`) left out of a customer's page. */
export function customer(text: string): string {
  return (
    text
      .replace(/^\d\d §[\d.]+(?:, §[\d.]+)*(?= for staff:)/, "")
      .replace(/^ for staff: ?(.)/, (_, c: string) => `For staff: ${c}`)
      .replace(/^\d\d §[\d.]+(?:, §[\d.]+)*[.:]\s*/, "")
      .replace(/^\d\d §[\d.]+(?:, §[\d.]+)*$/, "")
      .replace(
        /\s*\((?:`docs\/[\w.-]+`\s*)?(?:\d\d )?§[\d.]+(?:,\s*(?:\d\d )?§[\d.]+)*(?:,\s*D-\d+)*\)/g,
        "",
      )
      .replace(/\s*\(D-\d+\)/g, "")
      // A reference closing a parenthesis that says more: `(… sets, 09 §10)`.
      .replace(/,\s*(?:\d\d )?§[\d.]+(?:,\s*§[\d.]+)*\)/g, ")")
      .replace(/:\s*frames are in \d\d §[\d.]+ to §[\d.]+/g, "")
      .replace(/the binary protocol of 08 §8/g, "the binary realtime protocol")
      .replace(/protocol of `GET \/v1\/public\/rt` is 08 §8/g, "protocol of `GET /v1/public/rt`")
      .trim()
  );
}

function ref(value: unknown): string | undefined {
  const r = (value as Json | undefined)?.$ref;
  return typeof r === "string" ? r.split("/").pop() : undefined;
}

function cell(text: string): string {
  return text.replace(/\|/g, "\\|").replace(/\n+/g, " ");
}

const SCHEMES: Record<string, string> = {
  secretKey: "[secret key](../concepts/keys.md#secret-keys)",
  publishableKey: "[publishable key](../concepts/keys.md#publishable-keys)",
  publishableKeyQuery:
    "[publishable key](../concepts/keys.md#publishable-keys) in the `key` query parameter",
  adminKey: "[admin key](../concepts/keys.md#the-admin-key)",
  consoleSession: "console session",
  sessionToken: "[session token](../concepts/session-tokens.md) in `Cavea-Session`",
  holdToken: "[hold token](../concepts/holds.md) in `Cavea-Hold-Token`",
};

function scheme(s: string): string {
  const text = SCHEMES[s];
  if (text === undefined) throw new Error(`unknown security scheme ${s}`);
  return text;
}

function credential(op: Json): string {
  const alternatives = (op.security as Array<Record<string, unknown>> | undefined) ?? [];
  if (alternatives.length === 0) return "None";
  const names = alternatives.map((a) => Object.keys(a).toSorted());
  // A publishable key, with a session token when the event requires one.
  const withSession = names.find((n) => n.includes("sessionToken"));
  const without = names.find((n) => !n.includes("sessionToken"));
  if (names.length === 2 && withSession && without && withSession.length === without.length + 1) {
    const first = without.map(scheme).join(" and ");
    return `${first[0]?.toUpperCase()}${first.slice(1)}; also a ${scheme("sessionToken")}, required when the event sets \`require_session\`, verified whenever sent`;
  }
  const text = names.map((n) => n.map(scheme).join(" and ")).join(", or ");
  return `${text[0]?.toUpperCase()}${text.slice(1)}`;
}

interface Doc {
  paths: Record<string, Record<string, Json>>;
  components: {
    schemas: Record<string, Json>;
    parameters: Record<string, Json>;
    responses: Record<string, Json>;
    headers: Record<string, Json>;
  };
  info: Json;
}

function typeOf(schema: Json | undefined, depth = 0): string {
  if (schema === undefined) return "any";
  const name = ref(schema);
  if (name !== undefined) return `[${name}](schemas.md#${name})`;
  if (Array.isArray(schema.enum)) {
    return (schema.enum as unknown[]).map((v) => `\`${JSON.stringify(v)}\``).join(", ");
  }
  if (schema.const !== undefined) return `\`${JSON.stringify(schema.const)}\``;
  for (const key of ["oneOf", "anyOf"]) {
    const list = schema[key] as Json[] | undefined;
    if (list) return list.map((s) => typeOf(s, depth + 1)).join(" or ");
  }
  const types = Array.isArray(schema.type)
    ? (schema.type as string[])
    : [String(schema.type ?? "")];
  const out = types
    .filter((t) => t !== "")
    .map((t) => {
      if (t === "array") return `array of ${typeOf(schema.items as Json | undefined, depth + 1)}`;
      if (t === "string" && typeof schema.format === "string") return `string (${schema.format})`;
      return t;
    });
  return out.length > 0 ? out.join(" or ") : "any";
}

function bounds(schema: Json): string {
  const parts: string[] = [];
  if (schema.minLength !== undefined || schema.maxLength !== undefined) {
    parts.push(`${schema.minLength ?? 0} to ${schema.maxLength ?? "any number of"} characters`);
  }
  if (schema.minimum !== undefined || schema.maximum !== undefined) {
    parts.push(`${schema.minimum ?? "any"} to ${schema.maximum ?? "any"}`);
  }
  if (schema.minItems !== undefined || schema.maxItems !== undefined) {
    parts.push(`${schema.minItems ?? 0} to ${schema.maxItems ?? "any number of"} items`);
  }
  if (schema.default !== undefined) parts.push(`default \`${JSON.stringify(schema.default)}\``);
  return parts.join("; ");
}

/** A schema's fields as table rows, nested inline objects flattened as `a.b`. */
function fields(schema: Json, prefix = "", depth = 0): string[] {
  const props = (schema.properties as Record<string, Json> | undefined) ?? {};
  const required = new Set((schema.required as string[] | undefined) ?? []);
  const rows: string[] = [];
  for (const [name, prop] of Object.entries(props)) {
    const description = [customer(String(prop.description ?? "")), bounds(prop)]
      .filter((s) => s !== "")
      .join(" ");
    rows.push(
      `| \`${prefix}${name}\` | ${cell(typeOf(prop))} | ${required.has(name) ? "yes" : "no"} | ${cell(description)} |`,
    );
    if (depth < 2 && ref(prop) === undefined && prop.properties !== undefined) {
      rows.push(...fields(prop, `${prefix}${name}.`, depth + 1));
    }
    const items = prop.items as Json | undefined;
    if (depth < 2 && items && ref(items) === undefined && items.properties !== undefined) {
      rows.push(...fields(items, `${prefix}${name}[].`, depth + 1));
    }
  }
  return rows;
}

function fieldTable(schema: Json): string {
  const rows = fields(schema);
  if (rows.length === 0) return "";
  return ["| Field | Type | Required | Description |", "|---|---|---|---|", ...rows].join("\n");
}

function schemaText(doc: Doc, name: string): string {
  const schema = doc.components.schemas[name];
  if (schema === undefined) throw new Error(`no schema ${name}`);
  const parts = [`<h2 id="${name}">${name}</h2>`, ""];
  const description = customer(String(schema.description ?? ""));
  if (description !== "") parts.push(description, "");
  const table = fieldTable(schema);
  if (table !== "") parts.push(table, "");
  else parts.push(`Type: ${typeOf({ ...schema, description: undefined })}.`, "");
  return parts.join("\n");
}

function parameters(doc: Doc, o: Operation): Json[] {
  const list = [
    ...((doc.paths[o.path]?.parameters as unknown as Json[] | undefined) ?? []),
    ...((o.op.parameters as Json[] | undefined) ?? []),
  ];
  return list.map((p) => {
    const name = ref(p);
    if (name === undefined) return p;
    const found = doc.components.parameters[name];
    if (found === undefined) throw new Error(`no parameter ${name}`);
    return found;
  });
}

/** The error codes a shared response names, as the document lists them. */
function responseCodes(doc: Doc, response: Json): string[] {
  const name = ref(response);
  const r = name === undefined ? response : doc.components.responses[name];
  return [...String(r?.description ?? "").matchAll(/`([a-z_]+)`/g)].map((m) => m[1] ?? "");
}

function operationText(doc: Doc, o: Operation): string {
  const { op } = o;
  const id = String(op.operationId);
  const out: string[] = [];
  out.push(`<h2 id="${id}">${String(op.summary ?? id)}</h2>`, "");
  out.push(`\`${o.method.toUpperCase()} ${o.path}\` · operation \`${id}\``, "");
  const description = customer(String(op.description ?? ""));
  if (description !== "") out.push(description, "");

  const facts: string[] = [`| Credential | ${cell(credential(op))} |`];
  const permission = op["x-cavea-permission"];
  if (typeof permission === "string") {
    const roles = ((op["x-cavea-roles"] as string[] | undefined) ?? []).join(", ");
    const secret = (op.security as Array<Json> | undefined)?.some((a) => "secretKey" in a);
    facts.push(
      `| Permission | \`${permission}\`: ${roles}${secret ? " (a console session's role; a secret key may always)" : ""} |`,
    );
  }
  out.push("| | |", "|---|---|", ...facts, "");

  const params = parameters(doc, o);
  if (params.length > 0) {
    out.push("**Parameters**", "", "| Name | In | Required | Description |", "|---|---|---|---|");
    for (const p of params) {
      const schema = (p.schema as Json | undefined) ?? {};
      const text = [customer(String(p.description ?? "")), bounds(schema)]
        .filter((s) => s !== "")
        .join(" ");
      out.push(
        `| \`${String(p.name)}\` | ${String(p.in)} | ${p.required === true ? "yes" : "no"} | ${cell(text)} |`,
      );
    }
    out.push("");
  }

  const body = op.requestBody as Json | undefined;
  if (body !== undefined) {
    const content = (body.content as Record<string, Json> | undefined) ?? {};
    const types = Object.keys(content);
    const json = content["application/json"];
    const name = ref(json?.schema);
    if (json !== undefined && name !== undefined) {
      out.push(`**Body**: [${name}](schemas.md#${name}), \`application/json\``, "");
      const schema = doc.components.schemas[name];
      if (schema !== undefined) {
        const table = fieldTable(schema);
        if (table !== "") out.push(table, "");
      }
    } else {
      const text = customer(String(body.description ?? ""));
      out.push(
        `**Body**: ${types.map((t) => `\`${t}\``).join(", ")}${text ? `. ${text}` : ""}`,
        "",
      );
    }
  }

  const responses = op.responses as Record<string, Json>;
  const ok = Object.entries(responses).filter(([s]) => !/^[45]/.test(s));
  if (ok.length > 0) {
    out.push("**Replies**", "", "| Status | Meaning | Body |", "|---|---|---|");
    for (const [status, r] of ok) {
      const content = (r.content as Record<string, Json> | undefined) ?? {};
      const bodies = Object.entries(content).map(([type, c]) => {
        const name = ref(c.schema);
        return name === undefined ? `\`${type}\`` : `[${name}](schemas.md#${name})`;
      });
      out.push(
        `| \`${status}\` | ${cell(customer(String(r.description ?? "")))} | ${bodies.join(", ") || "none"} |`,
      );
    }
    out.push("");
  }

  const errors = Object.entries(responses).filter(([s]) => /^[45]/.test(s));
  if (errors.length > 0) {
    const listed = errors.map(([status, r]) => {
      const codes = responseCodes(doc, r);
      const inline = ref(r) === undefined ? customer(String(r.description ?? "")) : "";
      return `[\`${status}\`](../reference/errors.md#status-${status})${inline && codes.length === 0 ? ` ${inline}` : ""}`;
    });
    out.push(`**Error statuses**: ${listed.join(" · ")}`, "");
  }
  return out.join("\n");
}

function operations(doc: Doc): Operation[] {
  const out: Operation[] = [];
  for (const [path, item] of Object.entries(doc.paths)) {
    for (const [method, op] of Object.entries(item)) {
      if (method === "parameters") continue;
      out.push({ method, path, op });
    }
  }
  return out;
}

/** The generated pages: path (as if under `content/`) to Markdown. */
export function apiPages(doc: Doc): Map<string, string> {
  const pages = new Map<string, string>();
  const ops = operations(doc);
  const claimed = new Set<Operation>();
  const counts: string[] = [];
  for (const group of GROUPS) {
    const mine = ops.filter((o) => !claimed.has(o) && group.takes(o));
    for (const o of mine) claimed.add(o);
    const list = mine.map(
      (o) =>
        `| [${String(o.op.summary ?? o.op.operationId)}](#${String(o.op.operationId)}) | \`${o.method.toUpperCase()} ${o.path}\` |`,
    );
    pages.set(
      group.file,
      [
        `# ${group.title}`,
        "",
        group.intro,
        "",
        "| Operation | Route |",
        "|---|---|",
        ...list,
        "",
        ...mine.map((o) => operationText(doc, o)),
      ].join("\n"),
    );
    counts.push(`| [${group.title}](${group.file.slice(4)}) | ${mine.length} |`);
  }
  const left = ops.filter((o) => !claimed.has(o));
  if (left.length > 0) {
    throw new Error(
      `operations no group of apps/docs/src/api.ts claims: ${left.map((o) => o.op.operationId).join(", ")}`,
    );
  }

  const info = customer(String(doc.info.description ?? ""));
  pages.set(
    "api/index.md",
    [
      "# Server API",
      "",
      `This is every operation of the Cavea server's HTTP API, generated from the OpenAPI document the server itself serves at \`GET /v1/openapi.json\`, version ${String(doc.info.version)}. Read [Keys](../concepts/keys.md) first to see which credential goes where.`,
      "",
      "## Conventions",
      "",
      info,
      "",
      "- A **secret key** (`sk_…`) belongs in your backend only. A **publishable key** (`pk_…`) goes in the buyer's page. Send either as `Authorization: Bearer <key>`.",
      "- Errors carry a stable `code`, and often a `reason` in `details`: see [Error codes](../reference/errors.md). Limits on sizes and rates are in [Limits](../reference/limits.md).",
      "- The OpenAPI document is the machine-readable form of this reference: generate a client from it, or import it into an HTTP tool.",
      "",
      "## Operations by area",
      "",
      "| Area | Operations |",
      "|---|---|",
      ...counts,
      "| [Schemas](schemas.md) | every request and reply body |",
      "",
    ].join("\n"),
  );

  pages.set(
    "api/schemas.md",
    [
      "# Schemas",
      "",
      "Every request and reply body of the [server API](index.md), as the OpenAPI document defines it. Times are RFC 3339 in UTC.",
      "",
      ...Object.keys(doc.components.schemas)
        .toSorted()
        .map((name) => schemaText(doc, name)),
    ].join("\n"),
  );
  return pages;
}

/** The error codes the document's `Problem` schema enumerates. */
export function problemCodes(doc: Doc): string[] {
  const problem = doc.components.schemas.Problem as Json | undefined;
  const code = (problem?.properties as Record<string, Json> | undefined)?.code;
  return (code?.enum as string[] | undefined) ?? [];
}

export type { Doc };
