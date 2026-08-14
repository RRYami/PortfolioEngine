import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// POST /api/auth/register → create account; forwards the session Set-Cookie.
export async function POST(req: Request) {
  const body = await req.text();
  return forward(req, `${PTF_API_URL}/api/auth/register`, {
    method: "POST",
    body,
  });
}
