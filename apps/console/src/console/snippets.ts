// The quick start's snippets (09 §6, CON-011), filled with this workspace's
// values: the server's URL (the console's own origin), the workspace, and a
// publishable key when one is at hand. A secret key is never filled in:
// the backend snippets read it from an environment variable.

export interface SnippetValues {
  /** The server's URL: the page's own origin. */
  origin: string;
  workspace: { key: string; name: string };
  /** A publishable key's value, or `null` for a placeholder. */
  publishable: string | null;
}

export interface Snippet {
  id: "create" | "page" | "book";
  language: "shell" | "ts";
  code: string;
}

/** What stands for a publishable key the console does not have. */
export const PUBLISHABLE_PLACEHOLDER = "pk_…";
/** What stands for the secret key: an environment variable of the backend. */
export const SECRET_PLACEHOLDER = "$CAVEA_SECRET_KEY";

const PUBLISHABLE = /^pk_[0-9a-f]{64}$/;
const SECRET = /^sk_[0-9a-f]{64}$/;

/** What a value pasted as a publishable key is. */
export function pastedKey(value: string): "publishable" | "secret" | "other" | "empty" {
  const v = value.trim();
  if (v === "") return "empty";
  if (PUBLISHABLE.test(v)) return "publishable";
  if (SECRET.test(v)) return "secret";
  return "other";
}

/** A string as a single-quoted shell or TypeScript literal. */
const quoted = (s: string) => `'${s.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`;
const shellQuoted = (s: string) => `'${s.replace(/'/g, `'"'"'`)}'`;

export function snippets(v: SnippetValues): Snippet[] {
  const origin = v.origin.replace(/\/+$/, "");
  const key = v.publishable !== null && PUBLISHABLE.test(v.publishable) ? v.publishable : null;
  // On one line: a name or key with a line break must not leave the
  // comment and become a command in a pasted snippet.
  const workspace = `${v.workspace.name} (${v.workspace.key})`.replace(
    /[\p{Cc}\p{Zl}\p{Zp}]+/gu,
    " ",
  );
  return [
    {
      id: "create",
      language: "shell",
      code: [
        `# Workspace: ${workspace}. A secret key acts in its own workspace.`,
        `# CAVEA_SECRET_KEY holds a secret key of this workspace, from your secrets store.`,
        `curl -X POST ${shellQuoted(`${origin}/v1/events`)} \\`,
        `  -H "Authorization: Bearer ${SECRET_PLACEHOLDER}" \\`,
        `  -H 'Content-Type: application/json' \\`,
        `  -d '{"key": "my-event", "chart": "my-chart"}'`,
      ].join("\n"),
    },
    {
      id: "page",
      language: "ts",
      code: [
        `import { createSeatMap, caveaServer } from '@cavea/renderer';`,
        ``,
        `const map = await createSeatMap(document.getElementById('map')!, {`,
        `  state: caveaServer({`,
        `    baseUrl: ${quoted(origin)},`,
        `    publishableKey: ${quoted(key ?? PUBLISHABLE_PLACEHOLDER)},`,
        `    event: 'my-event',`,
        `  }),`,
        `  onSelectionChange: (selection) => cart.update(selection.objects, selection.holdToken),`,
        `});`,
      ].join("\n"),
    },
    {
      id: "book",
      language: "shell",
      code: [
        `# At checkout, from your backend: book what the buyer's hold token holds.`,
        `curl -X POST ${shellQuoted(`${origin}/v1/events/my-event/book`)} \\`,
        `  -H "Authorization: Bearer ${SECRET_PLACEHOLDER}" \\`,
        `  -H 'Content-Type: application/json' \\`,
        `  -d '{"hold_token": "<the hold token>", "order_ref": "<your order id>"}'`,
      ].join("\n"),
    },
  ];
}
