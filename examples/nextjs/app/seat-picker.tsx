"use client";
// The buyer's side: the map, what they have chosen, and checkout. The map
// is drawn in the browser only; on the server this renders an empty box.
import { SeatMap } from "@cavea/react";
import { caveaServer, type Selection } from "@cavea/renderer";
import { useMemo, useState } from "react";
import { PRICES, pounds } from "../lib/prices";

// NEXT_PUBLIC_ values are built into the page: only ever public ones here.
const SERVER = process.env.NEXT_PUBLIC_CAVEA_URL ?? "";
const PUBLISHABLE_KEY = process.env.NEXT_PUBLIC_CAVEA_PUBLISHABLE_KEY ?? "";
const EVENT = process.env.NEXT_PUBLIC_CAVEA_EVENT ?? "";
const pricing = Object.entries(PRICES).map(([category, price]) => ({ category, price }));

interface Order {
  orderId: string;
  seats: string[];
  total: number;
  status: string;
}

async function post(path: string, body?: unknown): Promise<Order> {
  const reply = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const json = (await reply.json()) as Order & { error?: string };
  if (!reply.ok) throw new Error(json.error ?? `HTTP ${reply.status}`);
  return json;
}

const MESSAGES: Record<string, string> = {
  hold_expired: "Your seats were released before payment finished. You have not been charged; please choose again.",
  nothing_held: "Choose at least one seat first.",
};

export function SeatPicker() {
  // Made once: a new adapter would make a new map.
  const state = useMemo(() => caveaServer({ baseUrl: SERVER, publishableKey: PUBLISHABLE_KEY, event: EVENT }), []);
  const [selection, setSelection] = useState<Selection | null>(null);
  // While a seat is being held, it is not the buyer's yet.
  const [holding, setHolding] = useState(false);
  const [order, setOrder] = useState<Order | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const run = async (step: () => Promise<Order>) => {
    setBusy(true);
    setProblem(null);
    try {
      setOrder(await step());
    } catch (error) {
      const code = error instanceof Error ? error.message : "";
      setProblem(MESSAGES[code] ?? "Something went wrong. Please try again.");
      if (code === "hold_expired") setOrder(null);
    } finally {
      setBusy(false);
    }
  };
  const seats = selection?.objects ?? [];
  const holdToken = selection?.holdToken ?? null;
  const shown = seats.reduce((sum, seat) => sum + (seat.price ?? 0), 0);

  return (
    <div className="picker">
      <SeatMap
        state={state}
        pricing={pricing}
        currency="GBP"
        selection={{ max: 6 }}
        onSelectionChange={setSelection}
        onHoldPending={() => setHolding(true)}
        onHoldSettled={() => setHolding(false)}
        style={{ height: 520 }}
      />
      <aside>
        {order === null ? (
          <>
            <h2>Your seats</h2>
            <ul>
              {seats.map((seat) => (
                <li key={seat.label}>
                  {seat.label}, {seat.category.label}
                </li>
              ))}
            </ul>
            <p>{seats.length === 0 ? "Choose seats on the map." : `About ${pounds(shown)}`}</p>
            <button
              type="button"
              disabled={busy || holding || seats.length === 0 || holdToken === null}
              onClick={() => void run(() => post("/api/checkout", { holdToken }))}
            >
              Checkout
            </button>
          </>
        ) : order.status === "booked" ? (
          <>
            <h2>Booked</h2>
            <p>{order.seats.join(", ")}: enjoy the show.</p>
          </>
        ) : (
          <>
            <h2>Order</h2>
            <p>
              {order.seats.join(", ")}: {pounds(order.total)}
            </p>
            {/* FAKE payment: your payment provider's form goes here. */}
            <button type="button" disabled={busy} onClick={() => void run(() => post(`/api/orders/${order.orderId}/pay`))}>
              Pay {pounds(order.total)} (fake)
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => void run(() => post(`/api/orders/${order.orderId}/cancel`)).then(() => setOrder(null))}
            >
              Cancel
            </button>
          </>
        )}
        {problem !== null && <p role="alert">{problem}</p>}
      </aside>
    </div>
  );
}
