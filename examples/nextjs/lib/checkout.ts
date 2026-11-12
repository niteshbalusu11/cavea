// The checkout, on the server only: route handlers call these. The secret
// key is read here and never reaches the browser (it has no NEXT_PUBLIC_
// prefix). Orders are kept in memory to keep the example small: keep them
// in your database, with the signed-in buyer they belong to.
import { randomUUID } from "node:crypto";
import { Cavea, isCaveaError, type HoldTokenState } from "@cavea/server-sdk";
import { PRICES } from "./prices";

const EVENT = process.env.NEXT_PUBLIC_CAVEA_EVENT ?? "";
let client: Cavea | undefined;
/** The client, made at the first request (so that `next build` needs no key). */
function cavea(): Cavea {
  const secretKey = process.env.CAVEA_SECRET_KEY;
  if (secretKey === undefined || secretKey === "") throw new Error("CAVEA_SECRET_KEY is not set");
  client ??= new Cavea({ baseUrl: process.env.NEXT_PUBLIC_CAVEA_URL ?? "", secretKey });
  return client;
}

export interface Order {
  id: string;
  holdToken: string;
  seats: string[];
  total: number;
  paid: boolean;
  status: "awaiting_payment" | "booked" | "cancelled" | "refunded";
}
const orders = new Map<string, Order>();

/** What the buyer is told: an HTTP status and `{ error }`. */
export class Refusal extends Error {
  constructor(
    readonly status: number,
    readonly error: string,
  ) {
    super(error);
  }
}

export const findOrder = (id: string): Order => {
  const order = orders.get(id);
  if (order === undefined) throw new Refusal(404, "not_found");
  return order;
};

/** Prices what the hold token holds, from the server's answer, and makes an order. */
export async function checkout(holdToken: string): Promise<Order> {
  let held: HoldTokenState;
  try {
    held = await cavea().holdTokens.get(holdToken);
  } catch (error) {
    // Expired, or never existed: the seats are free again; nothing is charged.
    if (isCaveaError(error, "not_found")) throw new Refusal(409, "hold_expired");
    throw error;
  }
  if (held.event !== EVENT || held.objects.length === 0) throw new Refusal(409, "nothing_held");
  let total = 0;
  for (const seat of held.objects) {
    const price = PRICES[seat.category.key];
    // A category without a price is a set-up mistake: never sell it for nothing.
    if (price === undefined) throw new Error(`no price for category ${seat.category.key}`);
    total += price;
  }
  // The payment must finish before the hold ends: give it at least ten minutes.
  if (Date.parse(held.expires_at) - Date.now() < 10 * 60_000) {
    await cavea().holdTokens.extend(holdToken, { ttl_seconds: 600 });
  }
  const order: Order = {
    id: randomUUID(),
    holdToken,
    seats: held.objects.map((seat) => seat.label),
    total,
    paid: false,
    status: "awaiting_payment",
  };
  orders.set(order.id, order);
  return order;
}

/** Takes the payment, then books exactly the seats priced. Safe to call again. */
export async function pay(order: Order): Promise<Order> {
  if (order.status === "booked") return order;
  if (order.status !== "awaiting_payment") throw new Refusal(409, order.status);
  if (!order.paid) {
    await takePayment(order);
    order.paid = true;
  }
  try {
    // The same key on every try: a retry after a lost reply books nothing twice.
    await cavea().events.book(
      EVENT,
      { hold_token: order.holdToken, objects: order.seats, order_ref: order.id },
      { idempotencyKey: `book-${order.id}` },
    );
  } catch (error) {
    // The hold ended during the payment, or a paid-for seat was released.
    if (isCaveaError(error, "not_found") || isCaveaError(error, "object_not_held_by_token")) {
      await refundPayment(order);
      order.status = "refunded";
      throw new Refusal(409, "hold_expired");
    }
    // Anything else: the order stays paid and unbooked; paying again retries
    // the booking with the same key, and charges nothing more.
    throw error;
  }
  order.status = "booked";
  return order;
}

/** The buyer gave up: their seats go back on sale now, not when the hold ends. */
export async function cancel(order: Order): Promise<Order> {
  if (order.status !== "awaiting_payment" || order.paid) throw new Refusal(409, order.status);
  try {
    await cavea().holdTokens.release(order.holdToken);
  } catch (error) {
    if (!isCaveaError(error, "not_found")) throw error;
  }
  order.status = "cancelled";
  return order;
}

// FAKE: stands in for your payment provider. Authorise the amount here and
// capture it once the booking succeeds, if your provider can. Give the
// provider an idempotency key made from the order, as the booking has, so
// that a retry after a lost reply never charges or refunds twice.
async function takePayment(order: Order): Promise<void> {
  console.log(`FAKE payment of ${order.total} pence, key pay-${order.id}`);
}
async function refundPayment(order: Order): Promise<void> {
  console.log(`FAKE refund of ${order.total} pence, key refund-${order.id}`);
}
/** One step at a time per order: a second click on Pay waits for the first,
 * then sees the order booked or refunded, so nothing is charged or refunded twice. */
const queues = new Map<string, Promise<unknown>>();
export function serially<T>(order: Order, step: () => Promise<T>): Promise<T> {
  const next = (queues.get(order.id) ?? Promise.resolve()).catch(() => undefined).then(step);
  queues.set(order.id, next);
  return next;
}

/** The JSON answer for what a handler returned or threw. */
export async function answer(run: () => Promise<Order>): Promise<Response> {
  try {
    const order = await run();
    return Response.json({
      orderId: order.id,
      seats: order.seats,
      total: order.total,
      status: order.status,
    });
  } catch (error) {
    if (error instanceof Refusal)
      return Response.json({ error: error.error }, { status: error.status });
    // Log the request id, never the hold token: it can release the buyer's seats.
    const id = isCaveaError(error) ? ` (request ${error.requestId ?? "none"})` : "";
    console.error(`checkout: ${error instanceof Error ? error.message : String(error)}${id}`);
    return Response.json({ error: "internal_error" }, { status: 500 });
  }
}
