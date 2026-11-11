// Background images (docs/04-renderer.md §7, §22): the engine asks for one
// after a frame is drawn (`imageRequest`), by its content hash; the host
// fetches it, has the browser decode it (no decoder in the WASM), scales it
// down to the request's limits and hands the bitmap to `View.setImage`,
// which uploads it. At most two load at once; a failure is reported and the
// map carries on without the image.
import { httpError } from "./errors.ts";
import type { EngineEvent } from "./generated/engine.ts";

/** Where an image's bytes are: a URL to fetch, or the bytes. */
export type ImageAsset = string | URL | Blob;

/** Where the image with content hash `hash` (32 lowercase hex digits) is. */
export type ImageSource = (hash: string) => ImageAsset | Promise<ImageAsset>;

/** The engine's request, as `takeEvents` reports it. */
export type ImageRequest = Extract<EngineEvent, { type: "imageRequest" }>;

export interface ImageLoader {
  request(event: ImageRequest): void;
  /** Stops: loads in flight are aborted and their answers dropped. */
  release(): void;
}

/** What the loader takes from the browser; fakes in tests. */
export interface ImagePlatform {
  fetch(url: string | URL, init: RequestInit): Promise<Response>;
  createImageBitmap(source: Blob | ImageBitmap, options?: ImageBitmapOptions): Promise<ImageBitmap>;
}

const browser: ImagePlatform = {
  fetch: (url, init) => fetch(url, init),
  createImageBitmap: (source, options) => createImageBitmap(source, options),
};

/** Images loading at once, at most. */
const PARALLEL = 2;
const HASH = /^[0-9a-f]{32}$/;

/**
 * Answers the engine's image requests with `answer(id, bitmap)`, or
 * `answer(id, undefined)` and `failed(error)` when an image cannot be had.
 * The bitmap is closed after `answer` returns.
 */
export function createImageLoader(
  source: ImageSource | undefined,
  answer: (id: number, image: ImageBitmap | undefined) => void,
  failed: (error: unknown) => void,
  platform: ImagePlatform = browser,
): ImageLoader {
  const queue: ImageRequest[] = [];
  const abort = new AbortController();
  const { signal } = abort;
  let running = 0;

  async function load(request: ImageRequest): Promise<ImageBitmap> {
    // Never a URL from anything but a hash.
    if (!HASH.test(request.asset) || source === undefined) {
      throw new Error(`image ${request.asset}: no source (assets.image or the adapter's image())`);
    }
    const asset = await source(request.asset);
    let blob: Blob;
    if (asset instanceof Blob) {
      blob = asset;
    } else {
      const response = await platform.fetch(asset, { signal });
      if (!response.ok) {
        throw httpError(`image ${request.asset}`, response.status);
      }
      blob = await response.blob();
    }
    const bitmap = await platform.createImageBitmap(blob);
    const { width, height } = bitmap;
    const scale = Math.min(
      request.maxSide / Math.max(width, height),
      Math.sqrt(request.maxPixels / (width * height)),
    );
    if (scale >= 1) {
      return bitmap;
    }
    try {
      return await platform.createImageBitmap(bitmap, {
        resizeWidth: Math.max(1, Math.floor(width * scale)),
        resizeHeight: Math.max(1, Math.floor(height * scale)),
        resizeQuality: "high",
      });
    } finally {
      bitmap.close();
    }
  }

  function next(): void {
    while (running < PARALLEL && !signal.aborted) {
      const request = queue.shift();
      if (request === undefined) {
        return;
      }
      running++;
      load(request)
        .then(
          (bitmap) => {
            try {
              if (!signal.aborted) {
                answer(request.id, bitmap);
              }
            } catch (error) {
              failed(error);
            } finally {
              bitmap.close();
            }
          },
          (error: unknown) => {
            if (!signal.aborted) {
              answer(request.id, undefined);
              failed(error);
            }
          },
        )
        .finally(() => {
          running--;
          next();
        });
    }
  }

  return {
    request(event) {
      queue.push(event);
      next();
    },
    release() {
      abort.abort();
      queue.length = 0;
    },
  };
}
