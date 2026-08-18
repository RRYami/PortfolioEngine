export const PTF_API_URL =
  process.env.PTF_API_URL ?? "http://127.0.0.1:8080";

/**
 * Authenticated same-origin proxy helper.
 *
 * Forwards the browser's `Cookie` header upstream to the Rust API (session
 * auth) and forwards any upstream `Set-Cookie` headers (session establish /
 * clear) back to the browser. Every route handler that talks to the API must
 * use this — without it the session never round-trips.
 */
export async function forward(
  req: Request,
  upstreamUrl: string,
  init: { method?: string; body?: string } = {},
): Promise<Response> {
  const r = await fetch(upstreamUrl, {
    method: init.method ?? "GET",
    headers: {
      "content-type": "application/json",
      cookie: req.headers.get("cookie") ?? "",
    },
    body: init.body,
    cache: "no-store",
  });
  const headers = new Headers({ "content-type": "application/json" });
  for (const sc of r.headers.getSetCookie()) {
    headers.append("set-cookie", sc);
  }
  // 204/304 responses must have a null body per the Fetch spec.
  const body = r.status === 204 || r.status === 304 ? null : await r.text();
  return new Response(body, { status: r.status, headers });
}

/**
 * Read a JSON response, surfacing the API's `error` string on failure.
 *
 * The engine reports precise, actionable failures ("rate unavailable USD ->
 * EUR on 2010-01-04"); throwing a bare status code throws that away and
 * leaves the user with "HTTP 400".
 */
export async function getJson<T>(url: string): Promise<T> {
  const r = await fetch(url, { cache: "no-store" });
  const raw = await r.text();
  if (!r.ok) {
    let msg = "";
    try {
      const j = JSON.parse(raw);
      if (j && typeof j.error === "string") msg = j.error;
    } catch {
      // non-JSON body; fall back to the status
    }
    throw new Error(msg || `HTTP ${r.status}`);
  }
  return JSON.parse(raw) as T;
}
