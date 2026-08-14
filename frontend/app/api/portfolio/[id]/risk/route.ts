import { computeRiskPayload } from "@/app/lib/computeRisk";
import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// GET /api/portfolio/{id}/risk
//
// Proxies the Rust analytics engine (ptf-api), which folds the portfolio's
// transactions into a PortfolioState and runs compute_var. If the engine is
// unreachable, falls back to the labelled seeded mock so the UI still renders.
export async function GET(
  req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const { id } = await ctx.params;
  try {
    return await forward(req, `${PTF_API_URL}/api/portfolios/${id}/risk`);
  } catch {
    // Engine offline — serve the mock so the dashboard still works.
    return Response.json({ ...computeRiskPayload(id), mock: true });
  }
}
