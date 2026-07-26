"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import type { CSSProperties } from "react";
import type {
  HorizonRow,
  PerformancePayload,
  RelativeStats,
} from "@/app/lib/performanceTypes";
import type { PortfolioSummary } from "@/app/lib/portfolioTypes";
import { MINUS, nf, shortDate } from "@/app/lib/format";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const GAIN = "var(--gain)";
const LOSS = "var(--loss)";
const ACCENT = "var(--accent)";
const SORTINO = "#34d399";
const NEUTRAL = "#c5cad6";

/** `1.42`, dash for the n/a sentinel (exact 0 with no sample). */
function ratio(n: number): string {
  return nf(n, 2);
}
function signPct(n: number): string {
  return (n >= 0 ? "+" : MINUS) + nf(Math.abs(n), 2) + "%";
}
function signColor(n: number): string {
  return n >= 0 ? GAIN : LOSS;
}

const panelStyle: CSSProperties = { padding: "16px 18px" };

export interface PerformancePageProps {
  selectedId: string | null;
  selected: PortfolioSummary | null;
  refreshToken: number;
  onAddHolding: () => void;
}

export default function PerformancePage({
  selectedId,
  selected,
  refreshToken,
  onAddHolding,
}: PerformancePageProps) {
  const [data, setData] = useState<PerformancePayload | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Committed risk-free rate (annual %) + the editable draft in the input.
  const [rf, setRf] = useState(0);
  const [rfDraft, setRfDraft] = useState("0");
  const [hover, setHover] = useState<number | null>(null);

  const load = useCallback((id: string | null, rfPct: number) => {
    if (!id) return Promise.resolve();
    const q = `rf=${(rfPct / 100).toFixed(4)}`;
    return fetch(`/api/portfolio/${id}/performance?${q}`, { cache: "no-store" })
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.json();
      })
      .then((d: PerformancePayload) => {
        setData(d);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    void load(selectedId, rf);
  }, [selectedId, refreshToken, rf, load]);

  const commitRf = () => {
    const v = Number(rfDraft);
    setRf(Number.isFinite(v) ? v : 0);
  };

  const chart = useMemo(() => buildChart(data), [data]);

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
          Performance Ratios
        </div>
        <div className="pe-lbl" style={{ marginTop: 4 }}>
          {selected?.name ?? "—"} · risk-adjusted return · current book
        </div>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <span className="pe-lbl">Risk-free %</span>
          <Input
            value={rfDraft}
            onChange={(e) => setRfDraft(e.target.value)}
            onBlur={commitRf}
            onKeyDown={(e) => e.key === "Enter" && commitRf()}
            inputMode="decimal"
            style={{
              width: 66,
              height: "auto",
              padding: "6px 9px",
              fontSize: 12,
              textAlign: "right",
            }}
          />
        </div>
        {data && (
          <div className="mono" style={{ fontSize: 11.5, color: "#6b7280" }}>
            {data.asOf} · {data.sampleDays}d
          </div>
        )}
      </div>
    </div>
  );

  if (error) {
    return (
      <div style={{ flex: 1, padding: "18px 20px", minWidth: 0 }}>
        {header}
        <div className="pe-panel" style={{ padding: 40, color: LOSS }}>
          Failed to load performance: {error}
        </div>
      </div>
    );
  }
  if (!data) {
    return (
      <div style={{ flex: 1, padding: "18px 20px", minWidth: 0 }}>
        {header}
        <div className="pe-panel" style={{ padding: 40, color: "#6b7280" }}>
          Computing performance ratios…
        </div>
      </div>
    );
  }

  if (data.sampleDays < 2) {
    return (
      <div style={{ flex: 1, padding: "18px 20px", minWidth: 0 }}>
        {header}
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
          Not enough price history to compute ratios yet — add a holding to build
          an equity curve.
          <Button
            onClick={onAddHolding}
            style={{ background: ACCENT, color: "#fff", fontWeight: 700 }}
          >
            ＋ Add Holding
          </Button>
        </div>
      </div>
    );
  }

  const s = data.snapshot;
  const kpis: { label: string; value: string; color: string; sub: string }[] = [
    { label: "Sharpe", value: ratio(s.sharpe), color: signColor(s.sharpe), sub: "return / vol" },
    { label: "Sortino", value: ratio(s.sortino), color: signColor(s.sortino), sub: "return / downside" },
    { label: "Calmar", value: ratio(s.calmar), color: signColor(s.calmar), sub: "return / max DD" },
    { label: "Omega", value: ratio(s.omega), color: s.omega >= 1 ? GAIN : LOSS, sub: "gains / losses" },
    { label: "Ann Return", value: signPct(s.annReturnPct), color: signColor(s.annReturnPct), sub: "CAGR" },
    { label: "Ann Vol", value: nf(s.annVolPct, 1) + "%", color: NEUTRAL, sub: "annualized σ" },
    { label: "Max Drawdown", value: nf(s.maxDrawdownPct, 1) + "%", color: LOSS, sub: "peak to trough" },
  ];

  const hoverInfo =
    chart && hover != null && hover >= 0 && hover < chart.dates.length
      ? {
          date: chart.dates[hover],
          sharpe: chart.sharpe[hover],
          sortino: chart.sortino[hover],
          x: chart.x(hover),
        }
      : null;

  return (
    <div style={{ flex: 1, padding: "18px 20px", minWidth: 0 }}>
      {header}

      {/* snapshot KPI cards */}
      <div style={{ display: "grid", gridTemplateColumns: "repeat(7,1fr)", gap: 10 }}>
        {kpis.map((k) => (
          <div key={k.label} className="pe-panel" style={{ padding: "12px 13px" }}>
            <div className="pe-lbl" style={{ fontSize: 9.5 }}>{k.label}</div>
            <div
              className="mono"
              style={{
                fontSize: 19,
                fontWeight: 600,
                marginTop: 9,
                letterSpacing: "-.01em",
                whiteSpace: "nowrap",
                color: k.color,
              }}
            >
              {k.value}
            </div>
            <div style={{ fontSize: 10, marginTop: 6, color: "#6b7280" }}>{k.sub}</div>
          </div>
        ))}
      </div>

      {/* rolling ratios chart */}
      <div className="pe-panel" style={{ ...panelStyle, marginTop: 13 }}>
        <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between" }}>
          <div style={{ fontWeight: 700, fontSize: 13.5 }}>
            Rolling Ratios{" "}
            <span style={{ color: ACCENT, fontWeight: 600 }}>
              · {data.rolling.window}-day window
            </span>
          </div>
          <div style={{ display: "flex", gap: 18 }}>
            <Legend color={ACCENT} label="Sharpe" />
            <Legend color={SORTINO} label="Sortino" />
          </div>
        </div>
        {chart ? (
          <div
            style={{ position: "relative", marginTop: 12, cursor: "crosshair" }}
            onMouseMove={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              const f = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
              setHover(Math.round(f * (chart.dates.length - 1)));
            }}
            onMouseLeave={() => setHover(null)}
          >
            <svg
              viewBox={`0 0 ${chart.W} ${chart.H}`}
              preserveAspectRatio="none"
              style={{ width: "100%", height: 200, display: "block", overflow: "visible" }}
            >
              {chart.zeroY != null && (
                <line
                  x1={0}
                  y1={chart.zeroY}
                  x2={chart.W}
                  y2={chart.zeroY}
                  stroke="rgba(255,255,255,.14)"
                  strokeWidth={1}
                  strokeDasharray="3 4"
                />
              )}
              <path d={chart.sortinoPath} fill="none" stroke={SORTINO} strokeWidth={1.6} />
              <path d={chart.sharpePath} fill="none" stroke={ACCENT} strokeWidth={2} />
              {hoverInfo && (
                <line
                  x1={hoverInfo.x}
                  y1={0}
                  x2={hoverInfo.x}
                  y2={chart.H}
                  stroke={ACCENT}
                  strokeWidth={1}
                  opacity={0.5}
                />
              )}
            </svg>
            {hoverInfo && (
              <div
                style={{
                  position: "absolute",
                  top: 0,
                  left: `${(hoverInfo.x / chart.W) * 100}%`,
                  transform:
                    hoverInfo.x > chart.W * 0.6 ? "translateX(-108%)" : "translateX(8%)",
                  background: "#05060a",
                  border: "1px solid rgba(255,255,255,.13)",
                  borderRadius: 9,
                  padding: "8px 10px",
                  pointerEvents: "none",
                  whiteSpace: "nowrap",
                  boxShadow: "0 14px 34px rgba(0,0,0,.6)",
                }}
              >
                <div className="pe-lbl" style={{ color: NEUTRAL, marginBottom: 6 }}>
                  {shortDate(hoverInfo.date)}
                </div>
                <TipRow label="Sharpe" color={ACCENT} v={hoverInfo.sharpe} />
                <TipRow label="Sortino" color={SORTINO} v={hoverInfo.sortino} />
              </div>
            )}
          </div>
        ) : (
          <div style={{ padding: "28px 0", color: "#6b7280", fontSize: 12 }}>
            Not enough history for a rolling window yet.
          </div>
        )}
      </div>

      {/* horizon table + benchmark-relative stats */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "1.35fr 1fr",
          gap: 13,
          marginTop: 13,
        }}
      >
        <HorizonTable rows={data.byHorizon} />
        <RelativePanel relative={data.relative} rfPct={data.rfAnnualPct} />
      </div>

      {/* distribution stat strip */}
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(6,1fr)",
          gap: 10,
          marginTop: 13,
        }}
      >
        <MiniStat label="Win rate" value={nf(s.winRatePct, 1) + "%"} />
        <MiniStat label="Best day" value={signPct(s.bestDayPct)} color={GAIN} />
        <MiniStat label="Worst day" value={signPct(s.worstDayPct)} color={LOSS} />
        <MiniStat label="Downside σ" value={nf(s.downsideDevPct, 1) + "%"} />
        <MiniStat label="Skew" value={ratio(s.skew)} color={signColor(s.skew)} />
        <MiniStat label="Excess kurt." value={ratio(s.kurtosis)} />
      </div>
    </div>
  );
}

function Legend({ color, label }: { color: string; label: string }) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 11, color: "#9aa1b2" }}>
      <span style={{ width: 14, height: 3, borderRadius: 2, background: color }} />
      {label}
    </div>
  );
}

function TipRow({ label, color, v }: { label: string; color: string; v: number | null }) {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", gap: 16, fontSize: 10.5, marginTop: 3 }}>
      <span style={{ color: "#6b7280" }}>{label}</span>
      <span className="mono" style={{ color }}>
        {v == null ? "—" : nf(v, 2)}
      </span>
    </div>
  );
}

function MiniStat({ label, value, color }: { label: string; value: string; color?: string }) {
  return (
    <div className="pe-panel" style={{ padding: "11px 12px" }}>
      <div className="pe-lbl" style={{ fontSize: 9.5 }}>{label}</div>
      <div className="mono" style={{ fontSize: 14, fontWeight: 600, marginTop: 6, color: color ?? NEUTRAL }}>
        {value}
      </div>
    </div>
  );
}

function HorizonTable({ rows }: { rows: HorizonRow[] }) {
  const cell: CSSProperties = { padding: "8px 10px", textAlign: "right", fontSize: 11.5 };
  const head: CSSProperties = { padding: "8px 10px", textAlign: "right" };
  return (
    <div className="pe-panel" style={panelStyle}>
      <div style={{ fontWeight: 700, fontSize: 13.5, marginBottom: 8 }}>By horizon</div>
      <div style={{ overflowX: "auto" }}>
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 11.5 }}>
          <thead>
            <tr className="pe-lbl" style={{ borderBottom: "1px solid rgba(255,255,255,.08)" }}>
              <th style={{ ...head, textAlign: "left" }}>Window</th>
              <th style={head}>Sharpe</th>
              <th style={head}>Sortino</th>
              <th style={head}>Calmar</th>
              <th style={head}>Return</th>
              <th style={head}>Vol</th>
              <th style={head}>Max DD</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.label} className="pe-row" style={{ borderBottom: "1px solid rgba(255,255,255,.04)" }}>
                <td className="mono" style={{ ...cell, textAlign: "left", fontWeight: 600 }}>
                  {r.label}
                </td>
                <td className="mono" style={{ ...cell, color: signColor(r.sharpe) }}>{ratio(r.sharpe)}</td>
                <td className="mono" style={{ ...cell, color: signColor(r.sortino) }}>{ratio(r.sortino)}</td>
                <td className="mono" style={{ ...cell, color: signColor(r.calmar) }}>{ratio(r.calmar)}</td>
                <td className="mono" style={{ ...cell, color: signColor(r.annReturnPct) }}>{signPct(r.annReturnPct)}</td>
                <td className="mono" style={{ ...cell, color: NEUTRAL }}>{nf(r.annVolPct, 1)}%</td>
                <td className="mono" style={{ ...cell, color: LOSS }}>{nf(r.maxDrawdownPct, 1)}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function RelativePanel({ relative, rfPct }: { relative: RelativeStats | null; rfPct: number }) {
  if (!relative) {
    return (
      <div className="pe-panel" style={panelStyle}>
        <div style={{ fontWeight: 700, fontSize: 13.5, marginBottom: 8 }}>vs Benchmark</div>
        <div style={{ color: "#9aa1b2", fontSize: 12, lineHeight: 1.5 }}>
          Benchmark data unavailable — showing self-contained ratios only. Relative
          stats (beta, alpha, information ratio, Treynor) appear once the benchmark
          series can be priced.
        </div>
      </div>
    );
  }
  const items: { label: string; value: string; color?: string; sub?: string }[] = [
    { label: "Beta", value: ratio(relative.beta), sub: "vs " + relative.benchmark },
    { label: "Alpha", value: signPct(relative.alphaPct), color: signColor(relative.alphaPct), sub: "annualized" },
    { label: "Info Ratio", value: ratio(relative.informationRatio), color: signColor(relative.informationRatio), sub: "active / TE" },
    { label: "Treynor", value: signPct(relative.treynor), color: signColor(relative.treynor), sub: "per unit β" },
    { label: "Correlation", value: ratio(relative.correlation), sub: "ρ" },
    { label: "R²", value: ratio(relative.rSquared), sub: "fit" },
    { label: "Up capture", value: nf(relative.upCapturePct, 0) + "%", color: relative.upCapturePct >= 100 ? GAIN : NEUTRAL },
    { label: "Down capture", value: nf(relative.downCapturePct, 0) + "%", color: relative.downCapturePct <= 100 ? GAIN : LOSS },
  ];
  return (
    <div className="pe-panel" style={panelStyle}>
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", marginBottom: 10 }}>
        <div style={{ fontWeight: 700, fontSize: 13.5 }}>
          vs {relative.benchmark}
        </div>
        <div className="pe-lbl">
          bench {signPct(relative.benchmarkAnnReturnPct)} · rf {nf(rfPct, 1)}%
        </div>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(4,1fr)", gap: 9 }}>
        {items.map((it) => (
          <div key={it.label}>
            <div className="pe-lbl" style={{ fontSize: 9 }}>{it.label}</div>
            <div className="mono" style={{ fontSize: 14.5, fontWeight: 600, marginTop: 5, color: it.color ?? NEUTRAL }}>
              {it.value}
            </div>
            {it.sub && <div style={{ fontSize: 9, color: "#4b5263", marginTop: 3 }}>{it.sub}</div>}
          </div>
        ))}
      </div>
    </div>
  );
}

interface ChartView {
  W: number;
  H: number;
  sharpePath: string;
  sortinoPath: string;
  zeroY: number | null;
  dates: string[];
  sharpe: (number | null)[];
  sortino: (number | null)[];
  x: (i: number) => number;
}

/** Map the rolling series onto SVG paths (nulls at the head are skipped). */
function buildChart(data: PerformancePayload | null): ChartView | null {
  const r = data?.rolling;
  if (!r) return null;
  const n = r.dates.length;
  const present = [...r.sharpe, ...r.sortino].filter(
    (v): v is number => v != null,
  );
  if (n < 2 || present.length < 2) return null;

  let min = Math.min(...present, 0);
  let max = Math.max(...present, 0);
  if (min === max) {
    min -= 1;
    max += 1;
  }
  const pad = (max - min) * 0.08;
  min -= pad;
  max += pad;

  const W = 800;
  const H = 220;
  const p = 6;
  const x = (i: number) => (n === 1 ? 0 : (i / (n - 1)) * W);
  const y = (v: number) => H - p - ((v - min) / (max - min)) * (H - 2 * p);

  const pathOf = (arr: (number | null)[]): string => {
    let d = "";
    let started = false;
    for (let i = 0; i < arr.length; i++) {
      const v = arr[i];
      if (v == null) continue;
      d += `${started ? "L" : "M"}${x(i).toFixed(1)} ${y(v).toFixed(1)} `;
      started = true;
    }
    return d.trim();
  };

  return {
    W,
    H,
    sharpePath: pathOf(r.sharpe),
    sortinoPath: pathOf(r.sortino),
    zeroY: min < 0 && max > 0 ? y(0) : null,
    dates: r.dates,
    sharpe: r.sharpe,
    sortino: r.sortino,
    x,
  };
}
