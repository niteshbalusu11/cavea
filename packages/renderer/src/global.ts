// The script-tag build's entry (docs/05-web-sdk.md §1): `dist/cavea.min.js`,
// a classic script that sets `window.Cavea` to the public API. `build.ts`
// bundles it as an IIFE in which `import.meta.url` is the script's own URL,
// read from `document.currentScript` while it runs, so the WASM and the atlas
// are fetched from beside it unless `assets.baseUrl` says otherwise.
import { pin } from "./browser.ts";
import * as Cavea from "./index.ts";

// Defined by `build.ts`: the SHA-384 of the WASM and the atlas built with it.
declare const CAVEA_INTEGRITY: { wasm: string; atlas: string };

pin(CAVEA_INTEGRITY);
Object.assign(globalThis, { Cavea });
