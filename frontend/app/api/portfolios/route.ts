import { PTF_API_URL, passthrough } from "@/app/lib/apiBase";

// GET  /api/portfolios       → list portfolios
// POST /api/portfolios       → create a portfolio
// Thin same-origin proxies to the Rust engine.

export async function GET() {
  try {
    const r = await fetch(`${PTF_API_URL}/api/portfolios`, {
      cache: "no-store",
    });
    return passthrough(r.status, await r.text());
  } catch {
    return Response.json([], { status: 200 });
  }
}

export async function POST(req: Request) {
  const body = await req.text();
  const r = await fetch(`${PTF_API_URL}/api/portfolios`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
  });
  return passthrough(r.status, await r.text());
}
