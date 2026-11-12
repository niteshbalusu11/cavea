// A seat picker in a React app: the map, and the selection lifted into the
// app, so that the summary (or a basket, or a router) can use it.
import { SeatMap } from "@cavea/react";
import { caveaServer, type Selection, type Theme } from "@cavea/renderer";
import { useMemo, useState } from "react";
import { Summary } from "./Summary";

// Public values only: Vite builds VITE_ variables into the page.
const SERVER = import.meta.env.VITE_CAVEA_URL;
const PUBLISHABLE_KEY = import.meta.env.VITE_CAVEA_PUBLISHABLE_KEY;
const EVENT = import.meta.env.VITE_CAVEA_EVENT;

// Prices to show, in pence, by the chart's category keys (the sample
// theatre's). Display only: your backend charges from its own table.
const pricing = [
  { category: "premium", price: 6500 },
  { category: "standard", price: 4500 },
  { category: "value", price: 2500 },
  { category: "access", price: 4500 },
];

// Your brand's colours, over the light or dark theme: CSS custom properties.
const BRAND = { "seat-selected-fill": "#7c3aed", "ring-focus": "#7c3aed", font: "Georgia, serif" };

export function App() {
  // Made once: a new adapter would make a new map.
  const state = useMemo(
    () => caveaServer({ baseUrl: SERVER, publishableKey: PUBLISHABLE_KEY, event: EVENT }),
    [],
  );
  const [selection, setSelection] = useState<Selection | null>(null);
  const [dark, setDark] = useState(false);
  // Theme changes apply to the live map; nothing is reloaded.
  const theme: Theme = { base: dark ? "dark" : "light", tokens: BRAND };

  return (
    <main style={{ maxWidth: 1000, margin: "0 auto", padding: 16, fontFamily: "Georgia, serif" }}>
      <h1>Opening night</h1>
      <label>
        <input type="checkbox" checked={dark} onChange={(e) => setDark(e.target.checked)} /> Dark
        map
      </label>
      <SeatMap
        state={state}
        pricing={pricing}
        currency="GBP"
        theme={theme}
        selection={{ max: 6 }}
        onSelectionChange={setSelection}
        style={{ height: 480, marginTop: 8 }}
      />
      <Summary selection={selection} />
    </main>
  );
}
