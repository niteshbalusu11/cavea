// A checkout backend for the Cavea seat map: Node's own HTTP server and
// @cavea/server-sdk, nothing else. Sessions and orders are kept in memory so
// the example stays small; a real backend keeps both in its database.
import { randomUUID } from "node:crypto";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { Cavea, isCaveaError, type HoldTokenState } from "@cavea/server-sdk";

function env(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === "") throw new Error(`${name} is not set`);
  return value;
}

const EVENT = env("CAVEA_EVENT");
// The secret key never leaves this process: not in a page, not in a log.
// Its id is needed only to sign buyers' session tokens.
const cavea = new Cavea({
  baseUrl: env("CAVEA_URL"),
  secretKey: env("CAVEA_SECRET_KEY"),
  keyId: env("CAVEA_SECRET_KEY_ID"),
});

// Your price list, in pence, by the category keys the chart's designer gave.
// Prices come from here and the server's answer, never from the page.
const PRICES: Record<string, number> = { premium: 6500, standard: 4500, value: 2500, access: 4500 };
const MAX_SEATS = 6;

interface Session {
  /** Who the buyer is. FAKE: your sign-in decides this. */
  buyer: string;
  holdToken?: string;
}
interface Order {
  id: string;
  session: string;
  holdToken: string;
  seats: string[];
  total: number;
  paid: boolean;
  status: "awaiting_payment" | "booked" | "cancelled" | "refunded";
}
const sessions = new Map<string, Session>();
const orders = new Map<string, Order>();

/** A failure the buyer's page is told about, as `{ error }` with a status. */
class Refusal extends Error {
  readonly status: number;
  readonly error: string;
  constructor(status: number, error: string) {
    super(error);
    this.status = status;
    this.error = error;
  }
}

// ------------------------------------------------------------------ routes

/** A hold token for this buyer, kept in their session: the page holds seats
 * under it with `caveaServer({ session: "continue", holdToken })`. */
async function holdToken(session: Session) {
  if (session.holdToken !== undefined) {
    try {
      const held = await cavea.holdTokens.get(session.holdToken);
      return { holdToken: session.holdToken, expiresAt: held.expires_at };
    } catch (error) {
      // Expired: make a new one below.
      if (!isCaveaError(error, "not_found")) throw error;
    }
  }
  const created = await cavea.holdTokens.create({ event: EVENT, max_objects: MAX_SEATS });
  session.holdToken = created.hold_token;
  return { holdToken: created.hold_token, expiresAt: created.expires_at };
}

/** A session token, for a page that makes its own hold token: the event can
 * then require one, so that every hold belongs to a buyer you know. */
async function seatSession(session: Session) {
  const token = await cavea.signSessionToken({
    sub: session.buyer,
    event: EVENT,
    maxObjects: MAX_SEATS,
  });
  return { token };
}

/** Prices what the hold token holds, from the server's reply, and makes an order. */
async function checkout(sid: string, session: Session, body: { holdToken?: unknown }) {
  const token = typeof body.holdToken === "string" ? body.holdToken : session.holdToken;
  if (token === undefined) throw new Refusal(400, "no_hold_token");
  let held: HoldTokenState;
  try {
    held = await cavea.holdTokens.get(token);
  } catch (error) {
    // Expired, or never existed: its seats are free again and nothing is charged.
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
    await cavea.holdTokens.extend(token, { ttl_seconds: 600 });
  }
  const order: Order = {
    id: randomUUID(),
    session: sid,
    holdToken: token,
    seats: held.objects.map((seat) => seat.label),
    total,
    paid: false,
    status: "awaiting_payment",
  };
  orders.set(order.id, order);
  return { orderId: order.id, seats: order.seats, total };
}

/** Takes the payment, then books exactly the seats priced. Safe to call again. */
async function pay(order: Order) {
  if (order.status === "booked") return { status: order.status, seats: order.seats };
  if (order.status !== "awaiting_payment") throw new Refusal(409, order.status);
  if (!order.paid) {
    await takePayment(order);
    order.paid = true;
  }
  try {
    // The same key on every try: a retry after a lost reply books nothing twice.
    await cavea.events.book(
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
    // Anything else (the server unreachable): the order stays paid and
    // unbooked, and paying again retries the booking with the same key.
    throw error;
  }
  order.status = "booked";
  return { status: order.status, seats: order.seats };
}

/** The buyer gave up: their seats go back on sale now, not when the hold ends. */
async function cancel(order: Order) {
  if (order.status !== "awaiting_payment" || order.paid) throw new Refusal(409, order.status);
  try {
    await cavea.holdTokens.release(order.holdToken);
  } catch (error) {
    // Already expired: the seats are free anyway.
    if (!isCaveaError(error, "not_found")) throw error;
  }
  order.status = "cancelled";
  return { status: order.status };
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
function serially<T>(order: Order, step: () => Promise<T>): Promise<T> {
  const next = (queues.get(order.id) ?? Promise.resolve()).catch(() => undefined).then(step);
  queues.set(order.id, next);
  return next;
}

// ------------------------------------------------------------------ HTTP

async function route(req: IncomingMessage, sid: string, session: Session): Promise<unknown> {
  const path = new URL(req.url ?? "/", "http://localhost").pathname;
  if (req.method !== "POST") throw new Refusal(405, "method_not_allowed");
  if (path === "/api/hold-token") return holdToken(session);
  if (path === "/api/seat-session") return seatSession(session);
  if (path === "/api/checkout") return checkout(sid, session, await json(req));
  const match = path.match(/^\/api\/orders\/([\w-]+)\/(pay|cancel)$/);
  const order = match === null ? undefined : orders.get(match[1] ?? "");
  // An order is only its own buyer's to pay or cancel.
  if (order === undefined || order.session !== sid) throw new Refusal(404, "not_found");
  return serially<unknown>(order, () => (match?.[2] === "pay" ? pay(order) : cancel(order)));
}

async function json(req: IncomingMessage): Promise<Record<string, unknown>> {
  let text = "";
  for await (const chunk of req) {
    text += chunk;
    if (text.length > 10_000) throw new Refusal(413, "too_large");
  }
  try {
    const body: unknown = JSON.parse(text || "{}");
    if (typeof body === "object" && body !== null) return body as Record<string, unknown>;
  } catch {
    // Answered below.
  }
  throw new Refusal(400, "invalid_json");
}

/** The buyer's session, by an HttpOnly cookie. FAKE sign-in: a new buyer per browser. */
function sessionOf(req: IncomingMessage, res: ServerResponse): [string, Session] {
  const sid = req.headers.cookie?.match(/(?:^|;\s*)sid=([\w-]+)/)?.[1];
  const known = sid === undefined ? undefined : sessions.get(sid);
  if (sid !== undefined && known !== undefined) return [sid, known];
  const fresh = randomUUID();
  const session = { buyer: `buyer-${randomUUID()}` };
  sessions.set(fresh, session);
  res.setHeader("set-cookie", `sid=${fresh}; HttpOnly; SameSite=Lax; Path=/`);
  return [fresh, session];
}

const server = createServer(async (req, res) => {
  const [sid, session] = sessionOf(req, res);
  let status = 200;
  let body: unknown;
  try {
    body = await route(req, sid, session);
  } catch (error) {
    if (error instanceof Refusal) {
      status = error.status;
      body = { error: error.error };
    } else {
      // Log the request id, never the hold token: it can release the buyer's seats.
      const id = isCaveaError(error) ? ` (request ${error.requestId ?? "none"})` : "";
      console.error(`${req.url}: ${error instanceof Error ? error.message : String(error)}${id}`);
      status = 500;
      body = { error: "internal_error" };
    }
  }
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
});

const port = Number(process.env.PORT ?? 3000);
server.listen(port, () => console.log(`listening on http://localhost:${port}`));
