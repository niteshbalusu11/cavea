// A customer's backend in miniature, against a running cavea-server: the
// packed `@cavea/server-sdk` installed as a dependency, type-checked in a
// strict project without the DOM library, compiled and run under Node.
// Prints one JSON line of what it saw.
import {
  Cavea,
  CaveaError,
  isCaveaError,
  type Chart,
  type HoldTokenCreated,
} from "@cavea/server-sdk";

const env = (name: string): string => {
  const value = process.env[name];
  if (value === undefined) {
    throw new Error(`${name} is not set`);
  }
  return value;
};
const baseUrl = env("CAVEA_URL");
const operator = new Cavea({ baseUrl, adminKey: env("CAVEA_ADMIN_KEY") });
const ws = await operator.workspaces.create({ key: `node-${process.pid}`, name: "Node fixture" });
const cavea = new Cavea({ baseUrl, secretKey: ws.secret_key.key, keyId: ws.secret_key.id });

const document: Record<string, unknown> = JSON.parse(env("CAVEA_CHART"));
const chart: Chart = await cavea.charts.create({ key: "theatre", name: "Theatre", document });
const { version } = await cavea.charts.publish(chart.key);
await cavea.events.create({ key: "show", chart: chart.key });

let free = 0;
const labels: string[] = [];
for await (const object of cavea.events.listObjects("show", { status: "free" })) {
  free++;
  if (labels.length < 2) {
    labels.push(object.label);
  }
}
const token: HoldTokenCreated = await cavea.holdTokens.create({ event: "show" });
await cavea.events.hold("show", { hold_token: token.hold_token, objects: labels });
const order = { hold_token: token.hold_token, order_ref: "A-1" };
const booked = await cavea.events.book("show", order, { idempotencyKey: "order-A-1" });
const replayed = await cavea.events.book("show", order, { idempotencyKey: "order-A-1" });

let code = "";
try {
  const other = await cavea.holdTokens.create({ event: "show" });
  await cavea.events.hold("show", { hold_token: other.hold_token, objects: labels });
} catch (error) {
  if (isCaveaError(error, "object_not_free")) {
    code = error.code;
  } else {
    throw error;
  }
}
const session = await cavea.signSessionToken({ sub: "buyer", event: "show" });
console.log(
  JSON.stringify({
    node: process.version,
    seats: version.stats.seats,
    free,
    booked: booked.objects.length,
    replayed: replayed.seq === booked.seq,
    conflict: code,
    session: session.split(".").length,
    error: new CaveaError({ operation: "book", status: 0, code: "network", message: "x" }).name,
  }),
);
