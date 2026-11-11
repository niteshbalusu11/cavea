// The benchmark page's chart, compiled in a worker with a WASM instance of its
// own, so the map's instance (whose memory S5 reports) never held the
// compiler's working set. Needs the `--demo` build.
import init, { View } from "../../../packages/renderer/wasm/cavea_web.js";

interface DemoStatics {
  demoChart(seats: number, seed: number, shape: number, preset: string, script: number): Uint8Array;
  demoLabels(ccf: Uint8Array): string[];
}

addEventListener("message", (e: MessageEvent<string>) => {
  void (async () => {
    try {
      await init({
        module_or_path: new URL(
          "../../../packages/renderer/wasm/cavea_web_bg.wasm",
          import.meta.url,
        ),
      });
      const demo = View as unknown as Partial<DemoStatics>;
      if (demo.demoChart === undefined || demo.demoLabels === undefined) {
        throw new Error("This WASM has no demo exports. Run `cargo xtask build-web --demo`.");
      }
      const ccf = demo.demoChart(0, 1, 0, e.data, 0);
      postMessage({ ccf, labels: demo.demoLabels(ccf) });
    } catch (error) {
      postMessage({ error: String(error) });
    }
  })();
});
