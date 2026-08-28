"use client";

import { useCallback, useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { useRouter } from "next/navigation";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import type { PortfolioSummary } from "@/app/lib/portfolioTypes";
import {
  AddHoldingDialog,
  AddOptionDialog,
  NewPortfolioDialog,
} from "@/app/components/PortfolioDialogs";
import RiskDashboard from "./RiskDashboard";
import PositionsPage from "./PositionsPage";

interface SessionUser {
  id: string;
  email: string;
}

type Page = "positions" | "risk";

interface NavItem {
  id: Page;
  label: string;
  icon: string;
  hint: string;
}

const NAV: NavItem[] = [
  { id: "positions", label: "Positions", icon: "▤", hint: "Holdings & lots" },
  { id: "risk", label: "Risk", icon: "◈", hint: "VaR & stress" },
];

export default function AppShell() {
  const router = useRouter();
  const [user, setUser] = useState<SessionUser | null>(null);
  const [portfolios, setPortfolios] = useState<PortfolioSummary[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [page, setPage] = useState<Page>("positions");
  const [newOpen, setNewOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [optionOpen, setOptionOpen] = useState(false);
  const [refreshToken, setRefreshToken] = useState(0);

  const loadPortfolios = useCallback(() => {
    return fetch("/api/portfolios", { cache: "no-store" })
      .then((r) => {
        // Expired/invalid session mid-use → back to the login page.
        if (r.status === 401) {
          router.replace("/login");
          return [];
        }
        return r.ok ? r.json() : [];
      })
      .then((list: PortfolioSummary[]) => {
        setPortfolios(list);
        setSelectedId((cur) => cur ?? list[0]?.id ?? null);
      })
      .catch(() => setPortfolios([]));
  }, [router]);

  // Session probe first: only load the workspace once the user is known.
  useEffect(() => {
    fetch("/api/auth/me", { cache: "no-store" })
      .then(async (r) => {
        if (r.status === 401) {
          router.replace("/login");
          return;
        }
        if (!r.ok) throw new Error(`me failed: ${r.status}`);
        setUser(await r.json());
        void loadPortfolios();
      })
      .catch(() => setPortfolios([]));
  }, [loadPortfolios, router]);

  async function logout() {
    await fetch("/api/auth/logout", { method: "POST" }).catch(() => {});
    router.replace("/login");
  }

  const selected = portfolios?.find((p) => p.id === selectedId) ?? null;

  const dialogs = (
    <>
      <NewPortfolioDialog
        open={newOpen}
        onOpenChange={setNewOpen}
        onCreated={(p) => {
          setPortfolios((prev) => [...(prev ?? []), p]);
          setSelectedId(p.id);
        }}
      />
      <AddHoldingDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        portfolioId={selectedId}
        defaultDate={selected?.inceptionDate}
        onAdded={() => setRefreshToken((t) => t + 1)}
      />
      <AddOptionDialog
        open={optionOpen}
        onOpenChange={setOptionOpen}
        portfolioId={selectedId}
        defaultDate={selected?.inceptionDate}
        onAdded={() => setRefreshToken((t) => t + 1)}
      />
    </>
  );

  if (portfolios === null) {
    return (
      <div className="pe-page">
        <div className="pe-card" style={{ padding: 40, color: "#6b7280" }}>
          Connecting to engine…
        </div>
      </div>
    );
  }

  if (portfolios.length === 0) {
    return (
      <div className="pe-page">
        <div
          className="pe-card"
          style={{
            padding: 48,
            display: "flex",
            flexDirection: "column",
            alignItems: "flex-start",
            gap: 14,
          }}
        >
          <div style={{ fontWeight: 800, fontSize: 18 }}>No portfolios yet</div>
          <div style={{ color: "#9aa1b2", fontSize: 13 }}>
            Create a book, add a few holdings, then explore positions and risk.
          </div>
          <Button
            onClick={() => setNewOpen(true)}
            style={{ background: "var(--accent)", color: "#fff", fontWeight: 700 }}
          >
            ＋ New portfolio
          </Button>
        </div>
        {dialogs}
      </div>
    );
  }

  const pageProps = {
    selectedId,
    selected,
    refreshToken,
    onAddHolding: () => setAddOpen(true),
    onAddOption: () => setOptionOpen(true),
  };

  return (
    <div className="pe-page">
      <div className="pe-card">
        {/* topbar */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "15px 22px",
            borderBottom: "1px solid rgba(255,255,255,.06)",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
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
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <Select
              value={selectedId ?? undefined}
              onValueChange={(v) => setSelectedId(v)}
            >
              <SelectTrigger
                className="h-auto gap-2 border-0"
                style={{
                  padding: "6px 12px",
                  borderRadius: 9,
                  background: "#14161d",
                  border: "1px solid rgba(255,255,255,.07)",
                  fontSize: 12,
                  color: "#c5cad6",
                }}
              >
                <span>{selected?.name ?? "Select portfolio"}</span>
                <span style={{ color: "#4b5263" }}>·</span>
                <span className="mono" style={{ color: "#9aa1b2" }}>
                  {selected?.baseCcy ?? "USD"}
                </span>
              </SelectTrigger>
              <SelectContent>
                {portfolios.map((p) => (
                  <SelectItem key={p.id} value={p.id}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              onClick={() => setNewOpen(true)}
              variant="ghost"
              title="New portfolio"
              className="h-auto"
              style={{
                padding: "6px 11px",
                borderRadius: 9,
                background: "#14161d",
                border: "1px solid rgba(255,255,255,.07)",
                color: "#9aa1b2",
                fontSize: 13,
              }}
            >
              ＋ New
            </Button>
            {user && (
              <>
                <span
                  className="mono"
                  title={user.email}
                  style={{
                    fontSize: 11,
                    color: "#6b7280",
                    maxWidth: 180,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {user.email}
                </span>
                <Button
                  onClick={() => void logout()}
                  variant="ghost"
                  title="Log out"
                  className="h-auto"
                  style={{
                    padding: "6px 11px",
                    borderRadius: 9,
                    background: "#14161d",
                    border: "1px solid rgba(255,255,255,.07)",
                    color: "#9aa1b2",
                    fontSize: 13,
                  }}
                >
                  Log out
                </Button>
              </>
            )}
          </div>
        </div>

        {/* body: sidebar + active page */}
        <div style={{ display: "flex" }}>
          <nav
            style={{
              width: 176,
              flexShrink: 0,
              borderRight: "1px solid rgba(255,255,255,.06)",
              padding: "16px 12px",
              display: "flex",
              flexDirection: "column",
              gap: 4,
            }}
          >
            <div className="pe-lbl" style={{ padding: "2px 10px 8px" }}>
              Workspace
            </div>
            {NAV.map((item) => {
              const active = page === item.id;
              const style: CSSProperties = {
                display: "flex",
                alignItems: "center",
                gap: 11,
                padding: "9px 11px",
                borderRadius: 10,
                cursor: "pointer",
                border: "1px solid transparent",
                background: active ? "rgba(99,102,241,.14)" : "transparent",
                borderColor: active ? "rgba(99,102,241,.30)" : "transparent",
                textAlign: "left",
                width: "100%",
                transition: "background .12s",
              };
              return (
                <button
                  key={item.id}
                  onClick={() => setPage(item.id)}
                  style={style}
                >
                  <span
                    style={{
                      fontSize: 15,
                      color: active ? "var(--accent)" : "#6b7280",
                      width: 18,
                      textAlign: "center",
                    }}
                  >
                    {item.icon}
                  </span>
                  <span style={{ display: "block" }}>
                    <span
                      style={{
                        display: "block",
                        fontSize: 13,
                        fontWeight: active ? 700 : 600,
                        color: active ? "#e8eaf0" : "#9aa1b2",
                      }}
                    >
                      {item.label}
                    </span>
                    <span
                      style={{ display: "block", fontSize: 10, color: "#4b5263" }}
                    >
                      {item.hint}
                    </span>
                  </span>
                </button>
              );
            })}
          </nav>

          {page === "positions" ? (
            <PositionsPage key={selectedId} {...pageProps} />
          ) : (
            <RiskDashboard key={selectedId} {...pageProps} />
          )}
        </div>
      </div>
      {dialogs}
    </div>
  );
}
