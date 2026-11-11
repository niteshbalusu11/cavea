// A server component page containing the client component that uses the
// React wrapper. `next build` renders it on the server, where importing
// `@cavea/react` and `@cavea/renderer` must touch no DOM; the browser then
// hydrates it and the map draws.
import Map from "./map.jsx";

export default function Page() {
  return (
    <main>
      <h1>Seat map</h1>
      <Map />
    </main>
  );
}
