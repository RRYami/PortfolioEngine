import { PTF_API_URL, forward } from "@/app/lib/apiBase";

// GET /api/auth/me → current session user, or 401 (session probe).
export async function GET(req: Request) {
  return forward(req, `${PTF_API_URL}/api/auth/me`);
}
