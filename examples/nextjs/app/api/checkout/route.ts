import { answer, checkout } from "../../../lib/checkout";

// The page sends its hold token in the body (never in a URL): it can release the seats.
export async function POST(request: Request): Promise<Response> {
  const body: unknown = await request.json().catch(() => null);
  const holdToken = (body as { holdToken?: unknown } | null)?.holdToken;
  if (typeof holdToken !== "string") {
    return Response.json({ error: "no_hold_token" }, { status: 400 });
  }
  return answer(() => checkout(holdToken));
}
