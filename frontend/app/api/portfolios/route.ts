import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// GET  /api/portfolios       → list portfolios
// POST /api/portfolios       → create a portfolio
// Thin same-origin proxies to the Rust engine (session cookie forwarded).

export async function GET(req: Request) {
  try {
    return await forward(req, `${PTF_API_URL}/api/portfolios`);
  } catch {
    return Response.json([], { status: 200 });
  }
}

export async function POST(req: Request) {
  const body = await req.text();
  return forward(req, `${PTF_API_URL}/api/portfolios`, {
    method: "POST",
    body,
  });
}
