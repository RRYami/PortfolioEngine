"use client";

import { useCallback, useEffect, useState } from "react";
import type { PositionsPayload } from "@/app/lib/positionsTypes";
import type { PortfolioSummary } from "@/app/lib/portfolioTypes";
import { comp } from "@/app/lib/format";
import { Button } from "@/components/ui/button";
import PositionsTable from "./PositionsTable";

export interface PositionsPageProps {
  selectedId: string | null;
  selected: PortfolioSummary | null;
  refreshToken: number;
  onAddHolding: () => void;
}

export default function PositionsPage({
  selectedId,
  selected,
  refreshToken,
  onAddHolding,
}: PositionsPageProps) {
  const [data, setData] = useState<PositionsPayload | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback((id: string | null) => {
    if (!id) return Promise.resolve();
    return fetch(`/api/portfolio/${id}/positions`, { cache: "no-store" })
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.json();
      })
      .then((d: PositionsPayload) => {
        setData(d);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    void load(selectedId);
  }, [selectedId, refreshToken, load]);

  const header = (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        marginBottom: 16,
        gap: 16,
      }}
    >
      <div>
        <div style={{ fontWeight: 800, fontSize: 16, letterSpacing: "-.01em" }}>
          Portfolio Positions
        </div>
        <div className="pe-lbl" style={{ marginTop: 4 }}>
          {selected?.name ?? "—"} · holdings &amp; tax lots
        </div>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
        {data && (
          <div style={{ textAlign: "right" }}>
            <div className="pe-lbl">Total value</div>
            <div
              className="mono"
              style={{ fontSize: 16, fontWeight: 600, marginTop: 2 }}
            >
              {comp(data.totalValue, data.baseCcy)}{" "}
              <span style={{ color: "#6b7280", fontSize: 11 }}>
                {data.baseCcy}
              </span>
            </div>
          </div>
        )}
        <Button
          onClick={onAddHolding}
          className="h-auto"
          style={{
            padding: "8px 14px",
            borderRadius: 10,
            background: "var(--accent)",
            color: "#fff",
            fontWeight: 700,
            fontSize: 12.5,
            boxShadow: "0 6px 18px rgba(99,102,241,.35)",
            cursor: "pointer",
          }}
        >
          ＋ Add Holding
        </Button>
      </div>
    </div>
  );

  return (
    <div style={{ flex: 1, padding: "18px 20px", minWidth: 0 }}>
      {header}
      {error ? (
        <div className="pe-panel" style={{ padding: 40, color: "var(--loss)" }}>
          Failed to load positions: {error}
        </div>
      ) : !data ? (
        <div className="pe-panel" style={{ padding: 40, color: "#6b7280" }}>
          Loading positions…
        </div>
      ) : data.positions.length === 0 ? (
        <div
          className="pe-panel"
          style={{
            padding: 40,
            color: "#9aa1b2",
            fontSize: 13,
            display: "flex",
            flexDirection: "column",
            alignItems: "flex-start",
            gap: 12,
          }}
        >
          This book has no holdings yet.
          <Button
            onClick={onAddHolding}
            style={{ background: "var(--accent)", color: "#fff", fontWeight: 700 }}
          >
            ＋ Add Holding
          </Button>
        </div>
      ) : (
        <PositionsTable positions={data.positions} baseCcy={data.baseCcy} />
      )}
    </div>
  );
}
