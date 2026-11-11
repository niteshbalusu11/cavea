// A second server component page, with a client component that imports
// `@cavea/renderer` itself, only in the browser, with a dynamic import.
import Map from "./map.jsx";

export default function Page() {
  return <Map />;
}
