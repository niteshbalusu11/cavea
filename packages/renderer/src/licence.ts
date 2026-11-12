// The licence mark (docs/12-security-licensing.md §7.3, §7.4; D-83): which
// mark a map shows, if any, decided once after the first interactive frame
// and never again. A licence problem never stops a sale (D-20): nothing here
// throws, waits on the network, or touches selection, holds or the realtime
// connection; every input is untrusted and checked for type and size. No
// anti-tamper measure (12 §7.5): the mark is a line of text.
import type { StateAdapter } from "./adapter.ts";
import type { TrustedKey } from "./generated/licence-keys.ts";

/** The mark shown: "Development licence", "Powered by Cavea", or none. */
export type Mark = "licenceDevelopment" | "licencePoweredBy" | null;

/**
 * What a licence says, wherever it came from: `licensed` (valid or in grace)
 * with the hosts it covers, `expired`, `invalid` (missing, or a file that
 * does not verify) with why, or `unknown`: nothing can be concluded (an
 * older server, a field this version does not read, a browser that cannot
 * check Ed25519), which never marks a customer (12 §7.4).
 */
export type Standing =
  | { kind: "licensed"; domains: string[] }
  | { kind: "expired" }
  | { kind: "invalid"; reason: string }
  | { kind: "unknown"; reason: string };

/** The largest licence file read, as the server's (`cavea_licence::MAX_BYTES`). */
const MAX_BYTES = 16 * 1024;
/** Domain patterns read from a server's answer, and the longest. */
const MAX_DOMAINS = 256;
const MAX_DOMAIN = 255;
const DAY_MS = 86_400_000;
/** What every licence signature covers before the payload. */
const CONTEXT = "cavea-licence-v1\0";

/**
 * The standing a map's adapter reports, set by `caveaServer` from the public
 * event's `licence` field, and by the console for its own maps. A symbol
 * this module keeps: not part of the public adapter interface.
 */
const REPORTED = Symbol("licence");
type Reporting = { [REPORTED]?: () => Standing };

/** Has `adapter` report `standing()` in place of the `licence` option. */
export function reportLicence<A extends StateAdapter>(adapter: A, standing: () => Standing): A {
  Object.defineProperty(adapter, REPORTED, { value: standing });
  return adapter;
}

/** What `adapter` reports, if it is one that does. */
export const reported = (adapter: StateAdapter): (() => Standing) | undefined =>
  (adapter as Reporting)[REPORTED];

/**
 * A console map (the event manager, the designer's preview): unmarked. The
 * console is the operator's tool, not a buyer's page, and shows the
 * licence's state in its own banners (09 §7). Workspace-only, never in the
 * published package.
 */
export const consoleMap = <A extends StateAdapter>(adapter: A): A =>
  reportLicence(adapter, () => ({ kind: "unknown", reason: "console" }));

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * The public event's `licence` field as the server sent it (12 §7.6):
 * absent (a server that predates licensing) or unreadable, `unknown`.
 */
export function fromServer(field: unknown): Standing {
  if (!isObject(field)) {
    return { kind: "unknown", reason: field === undefined ? "no licence field" : "unreadable" };
  }
  const { status, domains } = field;
  if (status === "expired") {
    return { kind: "expired" };
  }
  if (status === "unlicensed") {
    return { kind: "invalid", reason: "the server has no valid licence" };
  }
  if (status === "licensed" && Array.isArray(domains)) {
    return {
      kind: "licensed",
      domains: domains
        .slice(0, MAX_DOMAINS)
        .filter((d): d is string => typeof d === "string" && d.length <= MAX_DOMAIN),
    };
  }
  return { kind: "unknown", reason: "unreadable" };
}

/** A host name in its ASCII form, lowercase, without a final dot; `null` if it is not one. */
function ascii(name: string): string | null {
  try {
    const host = new URL(`http://${name}/`).hostname;
    return host.endsWith(".") ? host.slice(0, -1) : host;
  } catch {
    return null;
  }
}

const IPV4 = /^\d+\.\d+\.\d+\.\d+$/;

/**
 * Whether `host` (the page's, from its origin) is one the patterns cover: a
 * name exactly, or `*.` and a name for every name under it but not the name
 * itself. Case and a final dot do not count, ports are not part of a host,
 * internationalised names are compared in their ASCII form, and a wildcard
 * never covers an IP address. The page's own machine (`localhost`, its
 * subdomains, `127.0.0.0/8`, `[::1]`) is covered by any licence, so a
 * developer sees there what their production page shows (D-83); `null` (an
 * opaque origin: a file, a sandboxed frame) cannot be known and is covered.
 */
export function covers(host: string | null, patterns: readonly string[]): boolean {
  const h = host === null ? null : ascii(host);
  if (h === null) {
    return host === null;
  }
  const ip = IPV4.test(h) || h.startsWith("[");
  if (
    h === "localhost" ||
    h.endsWith(".localhost") ||
    h === "[::1]" ||
    (ip && h.startsWith("127."))
  ) {
    return true;
  }
  return patterns.some((pattern) => {
    const wild = pattern.startsWith("*.");
    const label = wild ? pattern.slice(2) : pattern;
    // Letters, digits, dots and hyphens only: nothing the URL parser would
    // read as a user, a port or a path.
    const name = /^[\p{L}\p{N}\p{M}.-]+$/u.test(label) ? ascii(label) : null;
    return name !== null && name !== "" && (wild ? !ip && h.endsWith(`.${name}`) : h === name);
  });
}

/**
 * The mark for `standing` on the page at `host` (12 §7.3): none while
 * licensed for this host; "Development licence" when licensed for others
 * or invalid; "Powered by Cavea" when expired; none when nothing can be
 * concluded. With the reason, for a development warning.
 */
export function decide(standing: Standing, host: string | null): { mark: Mark; why: string } {
  switch (standing.kind) {
    case "licensed":
      return covers(host, standing.domains)
        ? { mark: null, why: "" }
        : { mark: "licenceDevelopment", why: `the licence does not cover the host "${host}"` };
    case "expired":
      return { mark: "licencePoweredBy", why: "the licence has expired" };
    case "invalid":
      return { mark: "licenceDevelopment", why: `no valid licence: ${standing.reason}` };
    case "unknown":
      return { mark: null, why: "" };
  }
}

const invalid = (reason: string): Standing => ({ kind: "invalid", reason });

/** Standard base64 with padding, decoded only if it is the canonical encoding (as the server's). */
function base64(text: unknown): Uint8Array<ArrayBuffer> | null {
  if (typeof text !== "string" || !/^[A-Za-z0-9+/]*={0,2}$/.test(text) || text.length % 4 !== 0) {
    return null;
  }
  try {
    const binary = atob(text);
    return btoa(binary) === text ? Uint8Array.from(binary, (c) => c.charCodeAt(0)) : null;
  } catch {
    return null;
  }
}

/** `YYYY-MM-DD` of a day that exists, from 1970, as its first millisecond (UTC). */
function day(text: unknown): number | null {
  if (typeof text !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(text) || text < "1970") {
    return null;
  }
  const ms = Date.UTC(+text.slice(0, 4), +text.slice(5, 7) - 1, +text.slice(8, 10));
  return new Date(ms).toISOString().startsWith(text) ? ms : null;
}

const isU32 = (v: unknown): v is number =>
  Number.isInteger(v) && (v as number) >= 0 && (v as number) < 2 ** 32;
const strings = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((s) => typeof s === "string");

/**
 * The standing of a licence file's text, `licence` (12 §7.6), checked
 * against `keys` with `subtle` at `now` (ms): the server's rules, strict
 * about the envelope and the signature, lenient about unknown fields. A
 * browser without Ed25519 in WebCrypto cannot check the signature: `unknown`
 * ("treated as valid", 12 §7.4). Never throws.
 */
export async function check(
  licence: unknown,
  keys: readonly TrustedKey[],
  subtle: SubtleCrypto | undefined,
  now: number,
): Promise<Standing> {
  try {
    if (typeof licence !== "string") {
      return invalid("the licence option is not a string");
    }
    const bytes = new TextEncoder().encode(licence);
    if (bytes.length > MAX_BYTES) {
      return invalid("too_large");
    }
    let envelope: unknown;
    try {
      envelope = JSON.parse(licence);
    } catch {
      return invalid("malformed");
    }
    if (
      !isObject(envelope) ||
      !isU32(envelope["cavea_licence"]) ||
      !["key_id", "payload", "signature"].every((k) => typeof envelope[k] === "string")
    ) {
      return invalid("malformed");
    }
    if (envelope["cavea_licence"] !== 1) {
      return invalid("version");
    }
    const payload = base64(envelope["payload"]);
    const signature = base64(envelope["signature"]);
    if (payload === null || signature === null) {
      return invalid("malformed");
    }
    const trusted = keys.find((k) => k.id === envelope["key_id"]);
    const raw = base64(trusted?.key);
    if (trusted === undefined || raw === null) {
      return invalid("unknown_key");
    }
    if (signature.length !== 64) {
      return invalid("signature");
    }
    const message = new Uint8Array(CONTEXT.length + payload.length);
    message.set(new TextEncoder().encode(CONTEXT));
    message.set(payload, CONTEXT.length);
    const unverifiable: Standing = {
      kind: "unknown",
      reason: "this browser cannot verify Ed25519 signatures",
    };
    let verified: boolean;
    try {
      const ed25519 = { name: "Ed25519" };
      const key = await subtle?.importKey("raw", raw, ed25519, false, ["verify"]);
      if (subtle === undefined || key === undefined) {
        return unverifiable;
      }
      verified = await subtle.verify(ed25519, key, signature, message);
    } catch {
      return unverifiable;
    }
    if (!verified) {
      return invalid("signature");
    }
    let document: unknown;
    try {
      document = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(payload));
    } catch {
      return invalid("payload");
    }
    if (
      !isObject(document) ||
      !["licence_id", "customer", "plan"].every((k) => typeof document[k] === "string") ||
      !(document["features"] === undefined || strings(document["features"])) ||
      !(document["domains"] === undefined || strings(document["domains"])) ||
      !(
        document["max_workspaces"] === undefined ||
        document["max_workspaces"] === null ||
        isU32(document["max_workspaces"])
      ) ||
      !(document["grace_days"] === undefined || isU32(document["grace_days"])) ||
      typeof document["issued_at"] !== "string" ||
      typeof document["expires_at"] !== "string"
    ) {
      return invalid("payload");
    }
    const issued = day(document["issued_at"]);
    const expires = day(document["expires_at"]);
    if (issued === null || expires === null || expires < issued) {
      return invalid("dates");
    }
    // Valid through the end of `expires_at`, then `grace_days` of grace;
    // a clock before `issued_at` still counts as valid (12 §7.6).
    const end = expires + DAY_MS + DAY_MS * ((document["grace_days"] as number | undefined) ?? 0);
    return now < end
      ? {
          kind: "licensed",
          domains: ((document["domains"] as string[] | undefined) ?? []).map((d) =>
            d.trim().toLowerCase(),
          ),
        }
      : { kind: "expired" };
  } catch {
    return invalid("malformed");
  }
}
