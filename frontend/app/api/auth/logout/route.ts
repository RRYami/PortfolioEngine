import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// POST /api/auth/logout → destroy the session; forwards the cookie clear.
export async function POST(req: Request) {
  return forward(req, `${PTF_API_URL}/api/auth/logout`, { method: "POST" });
}
