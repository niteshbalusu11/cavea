// The canvas's images (docs/04-renderer.md §7, §25): the engine asks for an
// image after a frame is drawn (`imageRequest`), by its content hash; this
// loader gets its bytes from the host's source (a file just placed, kept in
// the page, or the server's URL), has the browser decode them (no decoder
// in the WASM), scales the bitmap down to the request's limits and hands it
// back. At most two load at once; a failure is reported and the canvas
// carries on without the image. Modelled on the buyer SDK's `images.ts`.

/** Where an image's bytes are: a URL to fetch, or the bytes. */
export type ImageAsset = string | URL | Blob;

/**
 * Where the image with content hash `hash` (32 lowercase hex digits) is:
 * `null` when the host has no way to get it.
 */
export type ImageSource = (hash: string) => ImageAsset | null;

/** The engine's request. */
export interface ImageRequest {
  type: "imageRequest";
  id: number;
  asset: string;
  maxSide: number;
  maxPixels: number;
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

/** Premultiplied, as the engine blends: WebGL ignores its own unpack
 * settings for a bitmap, so the bitmap must hold it. */
const PREMULTIPLY: ImageBitmapOptions = { premultiplyAlpha: "premultiply" };
const PARALLEL = 2;
const HASH = /^[0-9a-f]{32}$/;

export interface ImageLoader {
  request(event: ImageRequest): void;
  /** Stops: loads in flight are dropped when they end. */
  release(): void;
}

/** The scale (at most 1) that brings `width × height` within the limits. */
export function fitScale(
  width: number,
  height: number,
  maxSide: number,
  maxPixels: number,
): number {
  return Math.min(1, maxSide / Math.max(width, height), Math.sqrt(maxPixels / (width * height)));
}

/**
 * Answers image requests with `answer(id, bitmap)`, or `answer(id,
 * undefined)` and `failed(asset, error)` when an image cannot be had. The
 * bitmap is closed after `answer` returns.
 */
export function createImageLoader(
  source: ImageSource,
  answer: (id: number, image: ImageBitmap | undefined) => void,
  failed: (asset: string, error: unknown) => void,
  platform: ImagePlatform = browser,
): ImageLoader {
  const queue: ImageRequest[] = [];
  const abort = new AbortController();
  const { signal } = abort;
  let running = 0;

  async function load(request: ImageRequest): Promise<ImageBitmap> {
    // Never a URL from anything but a hash.
    const asset = HASH.test(request.asset) ? source(request.asset) : null;
    if (asset === null) throw new Error(`image ${request.asset}: no source`);
    let blob: Blob;
    if (asset instanceof Blob) {
      blob = asset;
    } else {
      const response = await platform.fetch(asset, { signal });
      if (!response.ok) throw new Error(`image ${request.asset}: HTTP ${response.status}`);
      blob = await response.blob();
    }
    const bitmap = await platform.createImageBitmap(blob, PREMULTIPLY);
    const { width, height } = bitmap;
    const scale = fitScale(width, height, request.maxSide, request.maxPixels);
    if (scale >= 1) return bitmap;
    try {
      return await platform.createImageBitmap(bitmap, {
        ...PREMULTIPLY,
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
      if (request === undefined) return;
      running++;
      load(request)
        .then(
          (bitmap) => {
            try {
              if (!signal.aborted) answer(request.id, bitmap);
            } finally {
              bitmap.close();
            }
          },
          (error: unknown) => {
            if (signal.aborted) return;
            answer(request.id, undefined);
            failed(request.asset, error);
          },
        )
        .catch((error: unknown) => failed(request.asset, error))
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
