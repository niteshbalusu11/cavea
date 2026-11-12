import { answer, cancel, findOrder, serially } from "../../../../../lib/checkout";

export async function POST(
  _: Request,
  ctx: { params: Promise<{ id: string }> },
): Promise<Response> {
  const { id } = await ctx.params;
  return answer(async () => {
    const order = findOrder(id);
    return serially(order, () => cancel(order));
  });
}
