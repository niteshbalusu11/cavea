// The background image loader (images.ts) against a fake fetch and decoder:
// the bound on loads at once, failures, a slow image, downscaling to the
// engine's limits, and destroy during a load.
import { describe, expect, test } from "bun:test";
import { createImageLoader, type ImagePlatform, type ImageRequest } from "./images.ts";

const hash = (c: string): string => c.repeat(32);

/** Lets the loader's promise chains run (reading a body takes a task). */
const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 5));

const request = (id: number, c: string, maxSide = 4096, maxPixels = 1 << 22): ImageRequest => ({
  type: "imageRequest",
  id,
  asset: hash(c),
  maxSide,
  maxPixels,
});

class FakeBitmap {
  closed = false;
  constructor(
    readonly width: number,
    readonly height: number,
  ) {}
  close(): void {
    this.closed = true;
  }
}

/** A platform whose fetches wait until `finish(url)`; images are `size` pixels. */
function fakePlatform(size: [number, number] = [100, 50]) {
  const pending = new Map<string, (response: Response) => void>();
  const fetched: string[] = [];
  const signals: AbortSignal[] = [];
  const bitmaps: FakeBitmap[] = [];
  const platform: ImagePlatform = {
    fetch(url, init) {
      fetched.push(String(url));
      if (init.signal) {
        signals.push(init.signal);
      }
      return new Promise((resolve) => pending.set(String(url), resolve));
    },
    createImageBitmap(_source, options) {
      const bitmap = new FakeBitmap(
        options?.resizeWidth ?? size[0],
        options?.resizeHeight ?? size[1],
      );
      bitmaps.push(bitmap);
      return Promise.resolve(bitmap as unknown as ImageBitmap);
    },
  };
  const finish = async (url: string, status = 200): Promise<void> => {
    pending.get(url)?.(new Response(status === 200 ? new Blob(["x"]) : null, { status }));
    pending.delete(url);
    await settle();
  };
  return { platform, fetched, signals, bitmaps, finish };
}

function loader(platform: ImagePlatform) {
  const answers: Array<[number, FakeBitmap | undefined, boolean]> = [];
  const failures: unknown[] = [];
  const images = createImageLoader(
    (h) => `https://cdn.test/${h}`,
    (id, image) => {
      const bitmap = image as unknown as FakeBitmap | undefined;
      answers.push([id, bitmap, bitmap?.closed ?? false]);
    },
    (error) => failures.push(error),
    platform,
  );
  return { images, answers, failures };
}

const url = (c: string): string => `https://cdn.test/${hash(c)}`;

describe("image loader", () => {
  test("loads two at a time; a slow image holds one slot only; a failure is reported", async () => {
    const fake = fakePlatform();
    const { images, answers, failures } = loader(fake.platform);
    images.request(request(1, "a"));
    images.request(request(2, "b"));
    images.request(request(3, "c"));
    await settle();
    expect(fake.fetched).toEqual([url("a"), url("b")]);
    // `a` is slow; `b` fails with a 404: `c` starts, and loads.
    await fake.finish(url("b"), 404);
    expect(answers).toEqual([[2, undefined, false]]);
    expect((failures[0] as { status?: number }).status).toBe(404);
    expect(fake.fetched).toEqual([url("a"), url("b"), url("c")]);
    await fake.finish(url("c"));
    await fake.finish(url("a"));
    // Answered open, then closed once the view has uploaded it.
    expect(answers.map(([id, image, closed]) => [id, image?.width, closed])).toEqual([
      [2, undefined, false],
      [3, 100, false],
      [1, 100, false],
    ]);
    expect(fake.bitmaps.every((b) => b.closed)).toBe(true);
    expect(failures).toHaveLength(1);
  });

  test("scales down to the request's side and pixels, keeping the aspect", async () => {
    const fake = fakePlatform([4000, 1000]);
    const { images, answers } = loader(fake.platform);
    images.request(request(1, "a", 2000));
    images.request(request(2, "b", 4096, 250_000));
    await settle();
    await fake.finish(url("a"));
    await fake.finish(url("b"));
    const sizes = answers.map(([, image]) => [image?.width, image?.height]);
    expect(sizes).toEqual([
      [2000, 500],
      [1000, 250],
    ]);
    // The full-size decodes are closed too.
    expect(fake.bitmaps.every((b) => b.closed)).toBe(true);
  });

  test("destroy during a load aborts it and answers nothing", async () => {
    const fake = fakePlatform();
    const { images, answers, failures } = loader(fake.platform);
    images.request(request(1, "a"));
    images.request(request(2, "b"));
    images.request(request(3, "c"));
    await settle();
    images.release();
    expect(fake.signals.every((s) => s.aborted)).toBe(true);
    await fake.finish(url("a"));
    await fake.finish(url("b"));
    expect(answers).toEqual([]);
    expect(failures).toEqual([]);
    expect(fake.fetched).not.toContain(url("c"));
    expect(fake.bitmaps.every((b) => b.closed)).toBe(true);
  });

  test("builds no URL from anything but a hash, and fails without a source", async () => {
    const fake = fakePlatform();
    const seen: string[] = [];
    const failures: unknown[] = [];
    const answers: number[] = [];
    const images = createImageLoader(
      (h) => {
        seen.push(h);
        return `https://cdn.test/${h}`;
      },
      (id) => answers.push(id),
      (error) => failures.push(error),
      fake.platform,
    );
    images.request({ ...request(1, "a"), asset: "../../../etc/passwd" });
    images.request({ ...request(2, "a"), asset: hash("A") });
    await settle();
    expect(seen).toEqual([]);
    expect(answers).toEqual([1, 2]);
    expect(failures).toHaveLength(2);
    const none = createImageLoader(
      undefined,
      (id) => answers.push(id),
      (e) => failures.push(e),
    );
    none.request(request(3, "a"));
    await settle();
    expect(answers).toEqual([1, 2, 3]);
  });

  test("takes the bytes from the source when it gives them", async () => {
    const fake = fakePlatform([10, 10]);
    const answers: number[] = [];
    const images = createImageLoader(
      () => Promise.resolve(new Blob(["x"])),
      (id, image) => answers.push(id, image?.width ?? 0),
      () => undefined,
      fake.platform,
    );
    images.request(request(7, "a"));
    await settle();
    expect(fake.fetched).toEqual([]);
    expect(answers).toEqual([7, 10]);
  });
});
