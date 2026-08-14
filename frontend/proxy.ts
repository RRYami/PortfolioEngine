import { NextResponse } from "next/server";
import type { NextRequest } from "next/server";

const PUBLIC_PATHS = new Set(["/login", "/register"]);

/**
 * Optimistic auth gate (Next.js "Proxy", formerly middleware).
 *
 * Checks only for the *presence* of the opaque `ptf_session` cookie — it
 * cannot be validated here. Real validation stays in the Rust API: any
 * data fetch with an invalid/expired session returns 401 and the client
 * redirects to /login.
 */
export function proxy(request: NextRequest) {
  const { pathname } = request.nextUrl;
  const isPublic = PUBLIC_PATHS.has(pathname);
  const hasSession = request.cookies.has("ptf_session");

  if (!hasSession && !isPublic) {
    return NextResponse.redirect(new URL("/login", request.url));
  }
  if (hasSession && isPublic) {
    return NextResponse.redirect(new URL("/", request.url));
  }
  return NextResponse.next();
}

export const config = {
  // Pages only — /api route handlers forward 401s themselves.
  matcher: ["/((?!api|_next/static|_next/image|favicon.ico).*)"],
};
