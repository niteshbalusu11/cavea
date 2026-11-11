// The quick start: filled with the workspace's values, never with a
// secret key (CON-011).
import { expect, test } from "bun:test";
import { pastedKey, PUBLISHABLE_PLACEHOLDER, SECRET_PLACEHOLDER, snippets } from "./snippets.ts";

const PK = `pk_${"ab".repeat(32)}`;
const SK = `sk_${"cd".repeat(32)}`;
const base = { origin: "https://tickets.example.com/", workspace: { key: "main", name: "Main" } };

test("the snippets carry the origin, the workspace and a publishable key", () => {
  const all = snippets({ ...base, publishable: PK })
    .map((s) => s.code)
    .join("\n");
  expect(all).toContain("'https://tickets.example.com/v1/events'");
  expect(all).toContain("baseUrl: 'https://tickets.example.com',");
  expect(all).toContain(`publishableKey: '${PK}'`);
  expect(all).toContain("Main (main)");
  expect(all).toContain(`Bearer ${SECRET_PLACEHOLDER}`);
});

test("a secret key never fills a snippet", () => {
  for (const publishable of [SK, "not a key", null]) {
    const all = snippets({ ...base, publishable })
      .map((s) => s.code)
      .join("\n");
    expect(all).not.toContain(SK);
    expect(all).toContain(`publishableKey: '${PUBLISHABLE_PLACEHOLDER}'`);
  }
  expect(pastedKey(` ${PK} `)).toBe("publishable");
  expect(pastedKey(SK)).toBe("secret");
  expect(pastedKey("pk_123")).toBe("other");
  expect(pastedKey("  ")).toBe("empty");
});

test("a workspace name cannot break out of a shell comment", () => {
  const code = snippets({
    ...base,
    workspace: { key: "main", name: "x\nrm -rf ~ y" },
    publishable: null,
  })[0]?.code;
  const lines = code?.split("\n") ?? [];
  expect(lines[0]).toBe(
    "# Workspace: x rm -rf ~ y (main). A secret key acts in its own workspace.",
  );
  expect(lines.filter((l) => l.includes("rm -rf"))).toHaveLength(1);
});
