// Base URL of the Rust analytics API (ptf-api). Server-side only.
export const PTF_API_URL =
  process.env.PTF_API_URL ?? "http://127.0.0.1:8080";

/** Forward a Response body + status, preserving JSON content type. */
export function passthrough(status: number, body: string): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "application/json" },
  });
}
