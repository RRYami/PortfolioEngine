import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// POST /api/portfolios/{id}/options → buy a listed option. The engine needs
// the underlying's ticker as well as the contract terms, because an option is
// risked as a function of its underlying rather than from its own history.
export async function POST(
  req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const { id } = await ctx.params;
  const body = await req.text();
  return forward(req, `${PTF_API_URL}/api/portfolios/${id}/options`, {
    method: "POST",
    body,
  });
}
