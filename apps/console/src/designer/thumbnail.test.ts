// A thumbnail never fails a publish: each step's failure is a version
// without one, after retries where the server did not answer (DSN-040).
import { expect, test } from "bun:test";
import { attachThumbnail, type Sent, type ThumbnailSteps } from "./thumbnail.ts";

function steps(over: Partial<ThumbnailSteps<string>> = {}) {
  const calls: string[] = [];
  const s: ThumbnailSteps<string> = {
    render: () => Promise.resolve(new Blob(["png"])),
    upload: () => (calls.push("upload"), Promise.resolve({ ok: "h" })),
    attach: (hash) => (calls.push(`attach ${hash}`), Promise.resolve({ ok: "v" })),
    wait: (ms) => (calls.push(`wait ${ms}`), Promise.resolve()),
    ...over,
  };
  return { s, calls };
}

test("rendered, uploaded and attached", async () => {
  const { s, calls } = steps();
  expect(await attachThumbnail(s)).toBe("v");
  expect(calls).toEqual(["upload", "attach h"]);
});

test("no answer: tried again after 2 and 4 s, then skipped", async () => {
  let answers: Sent<string>[] = [{ offline: true }, { ok: "h" }];
  const { s, calls } = steps({
    upload: () => (calls.push("upload"), Promise.resolve(answers.shift() ?? { offline: true })),
  });
  expect(await attachThumbnail(s)).toBe("v");
  expect(calls).toEqual(["upload", "wait 2000", "upload", "attach h"]);
  answers = [];
  calls.length = 0;
  expect(await attachThumbnail(s)).toBeNull();
  expect(calls).toEqual(["upload", "wait 2000", "upload", "wait 4000", "upload"]);
});

test("a refusal, a render that throws or draws nothing: no thumbnail, no rejection", async () => {
  const refused = steps({ attach: () => Promise.resolve({ failed: true }) });
  expect(await attachThumbnail(refused.s)).toBeNull();
  const thrown = steps({ render: () => Promise.reject(new Error("no WebGL")) });
  expect(await attachThumbnail(thrown.s)).toBeNull();
  const empty = steps({ render: () => Promise.resolve(null) });
  expect(await attachThumbnail(empty.s)).toBeNull();
  expect(empty.calls).toEqual([]);
  const broken = steps({ upload: () => Promise.reject(new Error("bug")) });
  expect(await attachThumbnail(broken.s)).toBeNull();
});
