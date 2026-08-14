"use client";

import { useState } from "react";
import type { ReactNode } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const accentBtn = {
  background: "var(--accent)",
  color: "#fff",
  fontWeight: 700,
} as const;

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label className="pe-lbl">{label}</Label>
      {children}
    </div>
  );
}

/** Shared login/register card in the dashboard's design system. */
export default function AuthForm({ mode }: { mode: "login" | "register" }) {
  const router = useRouter();
  const isLogin = mode === "login";
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    setError(null);
    if (!isLogin && password !== confirm) {
      setError("Passwords do not match");
      return;
    }
    setBusy(true);
    try {
      const r = await fetch(`/api/auth/${mode}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      if (!r.ok) {
        const data = await r.json().catch(() => null);
        setError(data?.error ?? `Request failed (HTTP ${r.status})`);
        return;
      }
      router.push("/");
    } catch {
      setError("Could not reach the server");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="pe-page"
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        minHeight: "100vh",
      }}
    >
      <div className="pe-card" style={{ width: 360, padding: 28 }}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 12,
            marginBottom: 6,
          }}
        >
          <div
            style={{
              width: 26,
              height: 26,
              borderRadius: 8,
              background: "var(--accent)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              boxShadow: "0 0 0 4px rgba(99,102,241,.12)",
            }}
          >
            <div
              style={{ width: 9, height: 9, borderRadius: 2, background: "#fff" }}
            />
          </div>
          <div style={{ fontWeight: 800, fontSize: 15, letterSpacing: "-.01em" }}>
            PortfolioEngine
          </div>
        </div>
        <div style={{ color: "#9aa1b2", fontSize: 13, marginBottom: 18 }}>
          {isLogin ? "Sign in to your account" : "Create your account"}
        </div>

        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <Field label="Email">
            <Input
              type="email"
              required
              autoComplete="email"
              placeholder="you@example.com"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              autoFocus
            />
          </Field>
          <Field label="Password">
            <Input
              type="password"
              required
              minLength={8}
              autoComplete={isLogin ? "current-password" : "new-password"}
              placeholder={isLogin ? "Your password" : "At least 8 characters"}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </Field>
          {!isLogin && (
            <Field label="Confirm password">
              <Input
                type="password"
                required
                minLength={8}
                autoComplete="new-password"
                placeholder="Repeat your password"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
              />
            </Field>
          )}

          {error && <p className="text-sm text-red-400">{error}</p>}

          <Button type="submit" disabled={busy} style={accentBtn}>
            {busy
              ? isLogin
                ? "Signing in…"
                : "Creating account…"
              : isLogin
                ? "Sign in"
                : "Create account"}
          </Button>
        </form>

        <div style={{ marginTop: 16, fontSize: 12, color: "#6b7280" }}>
          {isLogin ? (
            <>
              No account?{" "}
              <Link href="/register" style={{ color: "var(--accent)" }}>
                Create one
              </Link>
            </>
          ) : (
            <>
              Already have an account?{" "}
              <Link href="/login" style={{ color: "var(--accent)" }}>
                Sign in
              </Link>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
