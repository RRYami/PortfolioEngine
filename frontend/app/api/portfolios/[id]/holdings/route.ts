import { PTF_API_URL, passthrough } from "@/app/lib/apiBase";

// POST /api/portfolios/{id}/holdings → add a holding (ticker, qty, cost, ccy).
export async function POST(
  req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const { id } = await ctx.params;
  const body = await req.text();
  const r = await fetch(`${PTF_API_URL}/api/portfolios/${id}/holdings`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
  });
  return passthrough(r.status, await r.text());
}
