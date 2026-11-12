// Every refusal the account-security routes can send has words (CON-003):
// each route's own reasons (09 §10) are said by its screen, the general
// ones by `describe`, and a reason the OpenAPI document gains under one of
// these routes' codes fails here until it is placed.
import { expect, test } from "bun:test";
import document from "../../../../crates/cavea-server/openapi.json";
import { ApiError, type ErrorCode } from "../api/client.ts";
import { OPERATIONS, type OperationId } from "../api/generated.ts";
import { accountHelpRefusal, resetRefusal, totpRefusal } from "./refusals.ts";
import { next, START, type Flow } from "./signInFlow.ts";

type Pair = `${string} ${string}`;

/** Whether a step of the sign-in flow says something of its own. */
const flowSays = (from: Flow) => (e: ApiError) => {
  const after = next(from, { type: "refused", error: e });
  return after.notice !== null || after.fieldError !== null || after.wait !== null;
};

/** Each route's own reasons (`code reason`), and what its screen makes of
 * a refusal: true when it has words of its own for it. */
const ROUTES: Partial<Record<OperationId, { own: Pair[]; says: (e: ApiError) => boolean }>> = {
  consoleSignIn: {
    own: [
      "unauthorized credentials",
      "rate_limited backoff",
      "rate_limited sign_in",
      "overloaded password_hashing",
    ],
    says: flowSays(START),
  },
  consoleSignInSecondFactor: {
    own: [
      "unauthorized second_factor",
      "unauthorized challenge",
      "rate_limited backoff",
      "rate_limited sign_in",
    ],
    says: flowSays({ ...START, step: "code", challenge: "c".repeat(64) }),
  },
  consoleTotpEnrol: {
    own: [
      "forbidden credentials",
      "already_exists second_factor",
      "rate_limited backoff",
      "overloaded password_hashing",
    ],
    says: (e) => totpRefusal(e) !== null,
  },
  consoleTotpConfirm: {
    own: [
      "forbidden second_factor",
      "invalid_request no_second_factor",
      "already_exists second_factor",
      "rate_limited backoff",
    ],
    says: (e) => totpRefusal(e) !== null,
  },
  consoleTotpDisable: {
    own: [
      "forbidden credentials",
      "forbidden second_factor",
      "invalid_request no_second_factor",
      "rate_limited backoff",
      "overloaded password_hashing",
    ],
    says: (e) => totpRefusal(e, true) !== null,
  },
  resetMemberSecondFactor: {
    own: ["forbidden own_account", "forbidden other_workspaces", "not_found -"],
    says: (e) => accountHelpRefusal(e) !== null,
  },
  createMemberPasswordReset: {
    own: ["forbidden own_account", "forbidden other_workspaces", "not_found -"],
    says: (e) => accountHelpRefusal(e) !== null,
  },
  consolePasswordReset: {
    own: [
      "unauthorized reset_token",
      "invalid_request invalid_field",
      "rate_limited backoff",
      "rate_limited sign_in",
      "overloaded password_hashing",
    ],
    says: (e) => resetRefusal(e) !== null,
  },
};

/** Said by `describe` (errors.ts) on every screen: the session, the CSRF
 * and origin checks, a role or workspace the person no longer has (an admin
 * resetting an owner is the members screen's own). */
const GENERAL: Pair[] = [
  "unauthorized console_session",
  "unauthorized missing",
  "forbidden csrf",
  "forbidden origin",
  "forbidden role",
  "forbidden workspace",
];

/** Malformed requests: the console's own are well formed, so one of these
 * is a fault of ours, described in general with its request id. */
const MALFORMED: Pair[] = [
  "invalid_request body_too_large",
  "invalid_request body_unreadable",
  "invalid_request content_type",
  "invalid_request json_data",
  "invalid_request json_depth",
  "invalid_request json_syntax",
  "invalid_request path",
  "invalid_request query",
  "invalid_request workspace",
];

/** Reasons other routes send under the same codes (keys, seats, charts,
 * invitations, setup, the licence, the buyer's session, assets). */
const ELSEWHERE = new Set(
  [
    "already_exists sign_in",
    "forbidden instance_owner",
    "forbidden invitation_email",
    "forbidden key_kind",
    "forbidden licence_expired",
    "forbidden self_revocation",
    "forbidden session",
    "overloaded asset_memory",
    "overloaded compile_timeout",
    "unauthorized invalid",
    "unauthorized invitation",
    "unauthorized malformed",
    "unauthorized session_token",
    "unauthorized setup_token",
  ].concat(
    [
      "archived",
      "asset_dimensions",
      "asset_malformed",
      "asset_type",
      "bad_status",
      "detail_mismatch",
      "document",
      "extra_too_large",
      "idempotency_key",
      "idempotency_key_length",
      "if_match",
      "last_event_id",
      "last_owner",
      "no_objects",
      "order_ref_length",
      "revoked",
      "ticket_type_length",
      "token_already_exists",
      "too_many_objects",
      "ttl_out_of_range",
      "unpublished",
      "upgrade",
      "zero_object_limit",
    ].map((r) => `invalid_request ${r}`),
  ),
);

const STATUS: Partial<Record<string, number>> = {
  invalid_request: 400,
  unauthorized: 401,
  forbidden: 403,
  not_found: 404,
  already_exists: 409,
  rate_limited: 429,
  overloaded: 503,
};

const problem = (pair: Pair): ApiError => {
  const [code = "", reason = "-"] = pair.split(" ");
  const details: Record<string, unknown> = reason === "-" ? {} : { reason };
  // The one field these routes name: a reset link's new password.
  if (reason === "invalid_field") details["field"] = "password";
  return new ApiError(
    STATUS[code] ?? 500,
    code as ErrorCode,
    reason === "-" ? null : reason,
    details,
    "req_1",
    code === "rate_limited" ? 30 : null,
    "server text",
  );
};

const doc = document as unknown as {
  paths: Record<string, Record<string, { operationId?: string; responses: Record<string, unknown> }>>;
  components: {
    responses: Record<string, { description: string }>;
    schemas: { Problem: { "x-cavea-reasons": Record<string, string[]> } };
  };
};
const reasons = doc.components.schemas.Problem["x-cavea-reasons"];

/** The problem codes an operation declares, from its responses. */
function codesOf(op: OperationId): string[] {
  const { path, method } = OPERATIONS[op];
  const responses = doc.paths[path]?.[method.toLowerCase()]?.responses ?? {};
  return Object.values(responses).flatMap((r) => {
    const ref = (r as { $ref?: string }).$ref?.split("/").pop();
    const text = ref === undefined ? "" : (doc.components.responses[ref]?.description ?? "");
    return [...text.matchAll(/`(\w+)`/g)].map((m) => m[1] ?? "");
  });
}

test("each route's own refusals are in the document and its screen has words for them", () => {
  for (const [op, route] of Object.entries(ROUTES)) {
    const codes = codesOf(op as OperationId);
    for (const pair of route.own) {
      const [code = "", reason = "-"] = pair.split(" ");
      expect({ op, pair, declared: codes.includes(code) }).toEqual({ op, pair, declared: true });
      if (reason !== "-") {
        expect({ op, pair, listed: reasons[code]?.includes(reason) }).toEqual({
          op,
          pair,
          listed: true,
        });
      }
      expect({ op, pair, says: route.says(problem(pair)) }).toEqual({ op, pair, says: true });
    }
  }
});

test("every reason these routes' codes carry is placed: a new one fails until it is", () => {
  const placed = new Set<string>([
    ...Object.values(ROUTES).flatMap((r) => r.own),
    ...GENERAL,
    ...MALFORMED,
    ...ELSEWHERE,
  ]);
  const codes = new Set(Object.keys(ROUTES).flatMap((op) => codesOf(op as OperationId)));
  const unplaced = [...codes].flatMap((code) =>
    (reasons[code] ?? []).map((r) => `${code} ${r}`).filter((pair) => !placed.has(pair)),
  );
  expect(unplaced).toEqual([]);
});
