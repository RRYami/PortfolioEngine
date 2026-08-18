import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// POST /api/portfolios/{id}/sell → sell some or all of a holding.
export async function POST(
  req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const { id } = await ctx.params;
  const body = await req.text();
  return forward(req, `${PTF_API_URL}/api/portfolios/${id}/sell`, {
    method: "POST",
    body,
  });
}
