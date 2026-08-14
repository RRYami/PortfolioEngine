import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// POST /api/portfolios/{id}/holdings → add a holding (ticker, qty, cost, ccy).
export async function POST(
  req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const { id } = await ctx.params;
  const body = await req.text();
  return forward(req, `${PTF_API_URL}/api/portfolios/${id}/holdings`, {
    method: "POST",
    body,
  });
}
