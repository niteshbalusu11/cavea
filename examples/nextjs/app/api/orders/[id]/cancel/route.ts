import { answer, cancel, findOrder } from "../../../../../lib/checkout";

export async function POST(
  _: Request,
  ctx: { params: Promise<{ id: string }> },
): Promise<Response> {
  const { id } = await ctx.params;
  return answer(async () => cancel(findOrder(id)));
}
