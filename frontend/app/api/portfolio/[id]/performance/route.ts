import { PTF_API_URL, passthrough } from "@/app/lib/apiBase";

// GET /api/portfolio/{id}/performance?rf=&benchmark=
//
// Same-origin proxy to the Rust engine's performance-ratio view (Sharpe,
// Sortino, Calmar, … + benchmark-relative stats). Forwards the query string so
// the risk-free rate and benchmark selection reach the engine.
export async function GET(
  req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const { id } = await ctx.params;
  const qs = new URL(req.url).search;
  const r = await fetch(`${PTF_API_URL}/api/portfolios/${id}/performance${qs}`, {
    cache: "no-store",
  });
  return passthrough(r.status, await r.text());
}
