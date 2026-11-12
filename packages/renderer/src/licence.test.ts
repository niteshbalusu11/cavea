// The licence mark (12 §7.3, §7.4): the decision as a table, host matching
// as a property, and the browser's verification against licences issued by
// the real tool (`cargo xtask licence`) with a key made in this run. The key
// reaches the check through its `keys` argument, which the shipped map fills
// from the generated `TRUSTED_KEYS` only (`browser.ts`): no option or global
// can add one.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { customState } from "./adapter.ts";
import { fakeEnv, loaded, READY, sleep } from "./fakes.ts";
import { TRUSTED_KEYS, type TrustedKey } from "./generated/licence-keys.ts";
import { check, covers, decide, fromServer, reportLicence, type Standing } from "./licence.ts";
import { createMap } from "./seatmap.ts";

const DAY = 86_400_000;
const at = (day: string): number => Date.parse(`${day}T00:00:00Z`);
const licensed = (...domains: string[]): Standing => ({ kind: "licensed", domains });
/** The base64 line of a public-key file (after its comment). */
const publicKey = (folder: string): string =>
  readFileSync(join(folder, "public-key"), "utf8")
    .split("\n")
    .filter((l) => l !== "" && !l.startsWith("#"))
    .join("");

describe("the mark", () => {
  test.each<[string, Standing, string | null, string | null]>([
    ["licensed, on a covered host", licensed("tickets.acme.example"), "tickets.acme.example", null],
    ["licensed, under a wildcard", licensed("*.acme.example"), "box.acme.example", null],
    ["licensed, elsewhere", licensed("*.acme.example"), "acme.example", "licenceDevelopment"],
    ["licensed for no host", licensed(), "tickets.acme.example", "licenceDevelopment"],
    ["licensed, on localhost", licensed("tickets.acme.example"), "localhost", null],
    ["licensed, on 127.0.0.1", licensed(), "127.0.0.1", null],
    ["licensed, from a file", licensed("tickets.acme.example"), null, null],
    ["expired, on a covered host", { kind: "expired" }, "tickets.acme.example", "licencePoweredBy"],
    ["expired, on localhost", { kind: "expired" }, "localhost", "licencePoweredBy"],
    ["invalid", { kind: "invalid", reason: "x" }, "tickets.acme.example", "licenceDevelopment"],
    ["invalid, on localhost", { kind: "invalid", reason: "x" }, "localhost", "licenceDevelopment"],
    ["unknown", { kind: "unknown", reason: "x" }, "tickets.acme.example", null],
  ])("%s", (_, standing, host, mark) => {
    expect(decide(standing, host).mark).toBe(mark as never);
  });

  test("the server's field: read when it is one, never trusted further", () => {
    expect(fromServer({ status: "licensed", domains: ["a.example"] })).toEqual(
      licensed("a.example"),
    );
    expect(fromServer({ status: "expired", domains: [] }).kind).toBe("expired");
    expect(fromServer({ status: "unlicensed", domains: [] }).kind).toBe("invalid");
    // An older server, or a field this version cannot read: no mark.
    for (const field of [
      undefined,
      null,
      "licensed",
      [],
      {},
      { status: "trial" },
      { status: "licensed" },
      { status: "licensed", domains: "a.example" },
      { status: 1, domains: [] },
    ]) {
      expect(fromServer(field).kind).toBe("unknown");
    }
    // Hostile domains: other types dropped, the list and each entry bounded.
    const many = Array.from({ length: 10_000 }, (_, i) => `h${i}.example`);
    const read = fromServer({ status: "licensed", domains: [...many, 7, {}, "x".repeat(10_000)] });
    expect(read.kind === "licensed" && read.domains.length).toBe(256);
    const odd = fromServer({ status: "licensed", domains: [7, null, { a: 1 }, "ok.example"] });
    expect(odd).toEqual(licensed("ok.example"));
  });
});

describe("host matching", () => {
  // A small deterministic generator, so a failure names its case.
  let seed = 7;
  const next = (n: number): number => {
    seed = (seed * 1_103_515_245 + 12_345) % 2 ** 31;
    return seed % n;
  };
  // A letter first: a name whose last label is a number is an IPv4 address to the URL parser.
  const label = (): string =>
    "abcdefghijklmnopqrstuvwxyz"[next(26)] +
    Array.from({ length: next(8) }, () => "abcdefghijklmnopqrstuvwxyz0123456789-"[next(37)]).join(
      "",
    );
  const name = (): string => Array.from({ length: 1 + next(3) }, label).join(".");
  const shout = (s: string): string =>
    [...s].map((c) => (next(2) === 0 ? c.toUpperCase() : c)).join("");

  test("names and wildcards cover what they say, and nothing that merely ends alike", () => {
    for (let i = 0; i < 2_000; i += 1) {
      const base = name();
      const sub = `${label()}.${base}`;
      const deep = `${label()}.${sub}`;
      // A name covers itself whatever its case or a final dot.
      expect(covers(shout(base), [base])).toBe(true);
      expect(covers(`${base}.`, [shout(base)])).toBe(true);
      // `*.base` covers every name under it, not `base` itself.
      expect(covers(sub, [`*.${base}`])).toBe(true);
      expect(covers(deep, [`*.${shout(base)}`])).toBe(true);
      expect(covers(base, [`*.${base}`])).toBe(false);
      // No suffix or prefix tricks.
      expect(covers(`${label()}${base}`, [`*.${base}`])).toBe(false);
      expect(covers(`${label()}${base}`, [base])).toBe(false);
      expect(covers(`${base}.evil.org`, [base, `*.${base}`])).toBe(false);
      expect(covers(sub, [base])).toBe(false);
      expect(covers(base, [])).toBe(false);
    }
  });

  test("addresses, the page's own machine, ports, and internationalised names", () => {
    expect(covers("93.184.216.34", ["93.184.216.34"])).toBe(true);
    expect(covers("10.0.0.5", ["*.0.0.5"])).toBe(false);
    expect(covers("[2001:db8::1]", ["*.db8::1"])).toBe(false);
    for (const local of ["localhost", "app.localhost", "127.0.0.1", "127.8.9.10", "[::1]"]) {
      expect(covers(local, [])).toBe(true);
    }
    expect(covers("127.example.com", [])).toBe(false);
    expect(covers("localhost.example.com", [])).toBe(false);
    // Hosts come without a port; patterns that carry anything else are not names.
    expect(covers("a.example", ["a.example:443"])).toBe(false);
    expect(covers("evil.org", ["a.example@evil.org"])).toBe(false);
    expect(covers("evil.org", ["*.a.example/evil.org"])).toBe(false);
    expect(covers("evil.org", ["*"])).toBe(false);
    // The page's host is ASCII (the URL parser's); a pattern in Unicode is compared as ASCII.
    expect(covers("xn--bcher-kva.example", ["bücher.example"])).toBe(true);
    expect(covers("shop.xn--bcher-kva.example", ["*.BÜCHER.example"])).toBe(true);
    expect(covers("bücher.example", ["xn--bcher-kva.example"])).toBe(true);
  });
});

describe("a licence string, verified in the browser's WebCrypto", () => {
  const root = resolve(import.meta.dir, "../../..");
  const dir = mkdtempSync(join(process.env["TMPDIR"] ?? "/tmp", "cavea-licence-"));
  let key: TrustedKey;
  const files: Record<string, string> = {};

  function tool(...args: string[]): string {
    const run = Bun.spawnSync(["cargo", "xtask", "licence", ...args], {
      cwd: root,
      env: { ...process.env, CARGO_BUILD_JOBS: "8" },
    });
    if (run.exitCode !== 0) {
      throw new Error(run.stderr.toString());
    }
    return run.stdout.toString();
  }
  function issue(name: string, ...args: string[]): void {
    const out = join(dir, `${name}.json`);
    tool(
      "issue",
      "--key",
      join(dir, "signing-key"),
      "--id",
      `lic_test_${name}`,
      "--customer",
      "Test Customer",
      "--plan",
      "growth",
      ...args,
      "--out",
      out,
    );
    files[name] = readFileSync(out, "utf8");
  }

  beforeAll(() => {
    const made = /key id:\s+([0-9a-f]{16})/.exec(tool("keygen", "--out", dir));
    tool("keygen", "--out", join(dir, "other"));
    key = { id: made?.[1] ?? "", key: publicKey(dir) };
    expect(key.id).toHaveLength(16);
    issue(
      "valid",
      "--issued",
      "2027-01-10",
      "--expires",
      "2028-01-10",
      "--grace-days",
      "30",
      "--domain",
      "tickets.acme.example",
      "--domain",
      "*.acme.example",
    );
    issue("expired", "--issued", "2020-01-01", "--expires", "2021-01-01", "--grace-days", "0");
  }, 300_000);
  afterAll(() => rmSync(dir, { recursive: true, force: true }));

  const subtle = globalThis.crypto.subtle;
  const verify = (text: unknown, now = at("2027-06-01"), keys = [key]) =>
    check(text, keys, subtle, now);

  test("a licence the tool issued: its standing moves with the clock", async () => {
    expect(await verify(files["valid"])).toEqual({
      kind: "licensed",
      domains: ["tickets.acme.example", "*.acme.example"],
    });
    // Before its issue (a wrong clock) it is still valid; in grace still licensed.
    expect((await verify(files["valid"], at("2026-01-01"))).kind).toBe("licensed");
    expect((await verify(files["valid"], at("2028-01-10") + DAY - 1)).kind).toBe("licensed");
    expect((await verify(files["valid"], at("2028-02-09") + DAY - 1)).kind).toBe("licensed");
    expect((await verify(files["valid"], at("2028-02-10"))).kind).toBe("expired");
    expect((await verify(files["expired"])).kind).toBe("expired");
  });

  test("today's builds trust no key: every licence is invalid (unknown_key)", async () => {
    expect(TRUSTED_KEYS).toEqual([]);
    expect(await verify(files["valid"], at("2027-06-01"), [...TRUSTED_KEYS])).toEqual({
      kind: "invalid",
      reason: "unknown_key",
    });
    const other = publicKey(join(dir, "other"));
    expect(await verify(files["valid"], at("2027-06-01"), [{ id: key.id, key: other }])).toEqual({
      kind: "invalid",
      reason: "signature",
    });
  });

  test("any change to a byte of the file makes it invalid", async () => {
    const file = new TextEncoder().encode(files["valid"]);
    const body = file.length - 1;
    expect(file[body]).toBe(10);
    let tried = 0;
    for (let i = 0; i < body; i += 1) {
      const original = file[i] ?? 0;
      const replacements = new Set([
        original ^ 1,
        original ^ 0x20,
        original ^ 0x80,
        ...new TextEncoder().encode('"\\{}[],: 0Aa=+/'),
      ]);
      replacements.delete(original);
      for (const replacement of replacements) {
        const mutated = file.slice();
        mutated[i] = replacement;
        const text = new TextDecoder().decode(mutated);
        const standing = await verify(text);
        if (standing.kind !== "invalid") {
          throw new Error(`byte ${i} ${original} -> ${replacement}: ${JSON.stringify(standing)}`);
        }
        tried += 1;
      }
    }
    expect(tried).toBeGreaterThan(body * 10);
  });

  test("hostile input is invalid with a reason, and never throws", async () => {
    const envelope = JSON.parse(files["valid"] ?? "") as Record<string, unknown>;
    const withField = (field: string, value: unknown) =>
      JSON.stringify({ ...envelope, [field]: value });
    const cases: Array<[unknown, string]> = [
      [undefined, "the licence option is not a string"],
      [42, "the licence option is not a string"],
      [{ licence: files["valid"] }, "the licence option is not a string"],
      ["", "malformed"],
      ["null", "malformed"],
      ["[]", "malformed"],
      [`${"[".repeat(8_000)}${"]".repeat(8_000)}`, "malformed"],
      ["x".repeat(16 * 1024 + 1), "too_large"],
      [`{"pad":"${"é".repeat(9_000)}"}`, "too_large"],
      [withField("cavea_licence", 2), "version"],
      [withField("cavea_licence", "1"), "malformed"],
      [withField("key_id", 7), "malformed"],
      [withField("payload", "not base64!"), "malformed"],
      [withField("payload", `${String(envelope["payload"])} `), "malformed"],
      [withField("signature", String(envelope["signature"]).replace(/=+$/, "")), "malformed"],
      [withField("signature", btoa("x".repeat(63))), "signature"],
      [withField("key_id", "0000000000000000"), "unknown_key"],
      // Signed nonsense cannot be made without the key; a forged payload fails the signature.
      [withField("payload", btoa('{"licence_id":"x"}')), "signature"],
    ];
    for (const [input, reason] of cases) {
      expect(await verify(input)).toEqual({ kind: "invalid", reason });
    }
  });

  test("without Ed25519 in WebCrypto the licence is treated as valid; a malformed one is not", async () => {
    const refusing = {
      importKey: () => Promise.reject(new DOMException("Ed25519", "NotSupportedError")),
    } as unknown as SubtleCrypto;
    expect((await check(files["valid"], [key], refusing, at("2027-06-01"))).kind).toBe("unknown");
    expect((await check(files["valid"], [key], undefined, at("2027-06-01"))).kind).toBe("unknown");
    expect((await check("{}", [key], undefined, at("2027-06-01"))).kind).toBe("invalid");
    expect((await check(files["valid"], [], undefined, at("2027-06-01"))).kind).toBe("invalid");
  });
});

const markOf = (f: Awaited<ReturnType<typeof loaded>>): string | null =>
  [...f.root.walk()].find((n) => n.className === "w")?.textContent ?? null;
const reporting = (field: unknown) =>
  reportLicence(
    customState({ connect: async () => ({ kind: "map", unavailable: [] }), disconnect() {} }),
    () => fromServer(field),
  );

/** Draws the first frame with the state applied, then lets the check run. */
async function interactive(f: Awaited<ReturnType<typeof loaded>>): Promise<void> {
  f.drain(READY);
  expect(markOf(f)).toBeNull();
  f.attached()?.onFrame?.(0, 1);
  // Decided after the frame, never in it.
  expect(markOf(f)).toBeNull();
  await sleep(5);
}

describe("on the map", () => {
  test("a custom adapter without a licence: the Development mark, after the first interactive frame", async () => {
    const f = await loaded({
      state: customState({
        connect: async () => ({ kind: "map", unavailable: [] }),
        disconnect() {},
      }),
    });
    expect(f.root.getAttribute("data-w")).toBeNull();
    await interactive(f);
    expect(markOf(f)).toBe("Development licence");
    expect(f.root.getAttribute("data-w")).toBe("");
    // It never stands in the way: selecting still works.
    f.map.select(["A-1-1"]);
    expect(f.view.calls.some(([name]) => name === "select")).toBe(true);
  });

  test("what the server reports decides; the licence option is then ignored", async () => {
    const cases: Array<[unknown, string | null]> = [
      [{ status: "licensed", domains: ["tickets.example.com"] }, null],
      [{ status: "licensed", domains: ["other.example"] }, "Development licence"],
      [{ status: "expired", domains: [] }, "Powered by Cavea"],
      [{ status: "unlicensed", domains: [] }, "Development licence"],
      [undefined, null],
      ["garbage", null],
    ];
    for (const [field, mark] of cases) {
      const f = await loaded({ state: reporting(field), licence: "ignored" });
      await interactive(f);
      expect(markOf(f)).toBe(mark);
      f.map.destroy();
    }
  });

  test("nothing in the licence path can fail the map", async () => {
    // An option of the wrong type, a verifier that throws, an adapter whose report throws.
    const throwing = reportLicence(
      customState({ connect: async () => ({ kind: "map", unavailable: [] }), disconnect() {} }),
      () => {
        throw new Error("boom");
      },
    );
    for (const options of [
      { licence: 42 as unknown as string },
      { licence: "{" },
      { state: throwing },
    ]) {
      const f = await loaded(options);
      await interactive(f);
      f.map.select(["A-1-1"]);
      expect(f.map.getSelection().holdToken).toBeNull();
    }
    const f = fakeEnv();
    f.env.licence.subtle = () => {
      throw new Error("no crypto");
    };
    const promise = createMap(f.env, {} as HTMLElement, { chart: { url: "c.ccf" }, licence: "{}" });
    f.load();
    await promise;
  });

  test("destroyed before the check: nothing is shown and nothing throws", async () => {
    const f = await loaded();
    f.drain(READY);
    f.attached()?.onFrame?.(0, 1);
    f.map.destroy();
    await sleep(5);
    expect(markOf(f)).toBeNull();
  });
});
