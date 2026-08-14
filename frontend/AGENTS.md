<!-- BEGIN:nextjs-agent-rules -->
# This is NOT the Next.js you know

This version has breaking changes — APIs, conventions, and file structure may all differ from your training data. Read the relevant guide in `node_modules/next/dist/docs/` before writing any code. Heed deprecation notices.
<!-- END:nextjs-agent-rules -->

# Project conventions

## Auth proxying (do not bypass)
The Rust API uses session-cookie auth (`ptf_session`, HttpOnly). Every route
handler under `app/api/` **must** use `forward()` from `app/lib/apiBase.ts`:
it copies the browser's `Cookie` header upstream and forwards upstream
`Set-Cookie` headers back to the browser. The older `passthrough()` helper
drops both — using it for an authenticated call silently breaks the session.

- Auth state is probed client-side via `GET /api/auth/me` in `AppShell`;
  401 ⇒ redirect to `/login`. `proxy.ts` (Next 16 "Proxy"/middleware) does an
  optimistic cookie-presence redirect only — real validation is the API's 401.
- `/login` and `/register` are the only public pages; they share
  `app/components/AuthForm.tsx`.
