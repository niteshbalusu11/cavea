// The seat labels' glyph atlas (docs/04-renderer.md §4, §12): a PNG of a
// multi-channel signed distance field, baked by tools/glyph-atlas and shipped
// in ../assets. The browser decodes it, so the WASM carries no image decoder;
// the view gets the pixels once, as bytes.

import { httpError } from "./errors.ts";

/** Where to fetch the atlas from: by default the file next to this package. */
export type GlyphAtlasSource = string | URL;

/**
 * Fetches and decodes the glyph atlas to RGBA8 pixels for `View.setGlyphAtlas`.
 * The values are distances, not colours: no colour conversion or alpha
 * premultiplication is applied.
 */
export async function loadGlyphAtlas(
  source: GlyphAtlasSource = new URL("../assets/glyphs.png", import.meta.url),
): Promise<Uint8Array> {
  const response = await fetch(source);
  if (!response.ok) {
    throw httpError("glyph atlas", response.status);
  }
  const bitmap = await createImageBitmap(await response.blob(), {
    colorSpaceConversion: "none",
    premultiplyAlpha: "none",
  });
  const { width, height } = bitmap;
  const context = new OffscreenCanvas(width, height).getContext("2d");
  if (context === null) {
    throw new Error("glyph atlas: no 2D canvas to decode it");
  }
  context.drawImage(bitmap, 0, 0);
  bitmap.close();
  return new Uint8Array(context.getImageData(0, 0, width, height).data.buffer);
}
