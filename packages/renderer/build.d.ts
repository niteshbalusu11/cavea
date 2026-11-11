/** The ES module: what bundlers and `<script type="module">` load. */
export declare function buildModule(): Promise<string>;
/**
 * The script-tag build: an IIFE inside a function that reads its own URL
 * first (`document.currentScript` is only set while a classic script runs;
 * a module script or a loader that evaluates the text has none, and the
 * page's URL is used, so pass `assets.baseUrl` there).
 */
export declare function buildGlobal(): Promise<string>;
