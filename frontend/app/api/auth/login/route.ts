import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// POST /api/auth/login → verify credentials; forwards the session Set-Cookie.
export async function POST(req: Request) {
  const body = await req.text();
  return forward(req, `${PTF_API_URL}/api/auth/login`, {
    method: "POST",
    body,
  });
}
