import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// GET /api/portfolio/{id}/positions
//
// Thin same-origin proxy to the Rust engine's lightweight positions view
// (holdings valued at spot + their tax lots, no Monte-Carlo).
export async function GET(
  req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const { id } = await ctx.params;
  return forward(req, `${PTF_API_URL}/api/portfolios/${id}/positions`);
}
