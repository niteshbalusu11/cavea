import type { Selection } from "@cavea/renderer";

const pounds = (pence: number): string =>
  new Intl.NumberFormat("en-GB", { style: "currency", currency: "GBP" }).format(pence / 100);

/** What the buyer has chosen. Checkout is your backend's: see the Next.js example. */
export function Summary({ selection }: { selection: Selection | null }) {
  const seats = selection?.objects ?? [];
  if (seats.length === 0) return <p>Choose up to six seats on the map.</p>;
  const total = seats.reduce((sum, seat) => sum + (seat.price ?? 0), 0);
  return (
    <section aria-label="Your seats">
      <ul>
        {seats.map((seat) => (
          <li key={seat.label}>
            Row {seat.row}, seat {seat.seat} ({seat.category.label}): {pounds(seat.price ?? 0)}
          </li>
        ))}
      </ul>
      <p>Total: {pounds(total)}</p>
    </section>
  );
}
