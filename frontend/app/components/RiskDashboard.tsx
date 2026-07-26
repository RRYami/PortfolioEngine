"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import type { CSSProperties } from "react";
import type { Confidence, RiskPayload } from "@/app/lib/riskTypes";
import { derive, type DerivedView } from "@/app/lib/derive";
import { comp, MINUS } from "@/app/lib/format";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { PortfolioSummary } from "@/app/lib/portfolioTypes";

type ChartKind = "dist" | "hist" | "dd";

interface HoverState {
  chart: ChartKind;
  i: number;
  px: number;
  py: number;
  w: number;
}

interface TipRow {
  k: string;
  v: string;
  c: string;
}

interface Tip {
  left: string;
  top: string;
  transform: string;
  crossLeft: string;
  crossColor: string;
  title: string;
  titleColor: string;
  rows: TipRow[];
}


function place(cx: number, py: number, w: number) {
  const left = cx > w * 0.6;
  return {
    left: (left ? cx - 12 : cx + 12) + "px",
    top: Math.max(2, py - 6) + "px",
    transform: left ? "translateX(-100%)" : "none",
  };
}

function buildTip(v: DerivedView, conf: Confidence, h: HoverState): Tip | null {
  const { i, w, py } = h;
  const tot = v.tot;
  const pctOf = (x: number) => (x / tot) * 100;

  if (h.chart === "dist") {
    const { binLow: lo, bw, binCount: nb, counts, paths, tV, deepV } = v.dist;
    const low = lo + i * bw;
    const high = lo + (i + 1) * bw;
    const center = (low + high) / 2;
    const prob = (counts[i] / paths) * 100;
    let title = "Central outcomes";
    let tc = "var(--accent)";
    if (center <= deepV) {
      title = "Beyond " + (conf === 99 ? "99.75%" : "99%") + " tail";
      tc = "var(--lossDeep)";
    } else if (center <= tV) {
      title = "In " + v.confL + " tail";
      tc = "var(--loss)";
    }
    const cx = ((i + 0.5) / nb) * w;
    return {
      ...place(cx, py, w),
      crossLeft: cx + "px",
      crossColor: "var(--accent)",
      title,
      titleColor: tc,
      rows: [
        { k: "P&L", v: comp(low) + " … " + comp(high), c: "#e8eaf0" },
        {
          k: "% of book",
          v: pctOf(low).toFixed(2) + "% … " + pctOf(high).toFixed(2) + "%",
          c: "#9aa1b2",
        },
        { k: "Probability", v: prob.toFixed(1) + "% of paths", c: tc },
      ],
    };
  }

  if (h.chart === "hist") {
    const { v1d, v20d, dates } = v.histVar;
    const n = dates.length;
    const cx = (i / (n - 1)) * w;
    return {
      ...place(cx, py, w),
      crossLeft: cx + "px",
      crossColor: "var(--accent)",
      title: dates[i],
      titleColor: "#c5cad6",
      rows: [
        {
          k: "VaR 1-Day",
          v:
            MINUS + comp((v1d[i] / 100) * tot) + " · " + MINUS +
            v1d[i].toFixed(2) + "%",
          c: "var(--accent)",
        },
        {
          k: "VaR 20-Day",
          v:
            MINUS + comp((v20d[i] / 100) * tot) + " · " + MINUS +
            v20d[i].toFixed(2) + "%",
          c: "var(--loss)",
        },
      ],
    };
  }

  // drawdown
  const { series, dates } = v.dd;
  const n = dates.length;
  const d = series[i];
  const cx = (i / (n - 1)) * w;
  return {
    ...place(cx, py, w),
    crossLeft: cx + "px",
    crossColor: "var(--loss)",
    title: dates[i],
    titleColor: "#c5cad6",
    rows: [
      { k: "Drawdown", v: d.toFixed(2) + "%", c: "var(--loss)" },
      {
        k: "Depth",
        v: MINUS + comp((Math.abs(d) / 100) * tot),
        c: "var(--loss)",
      },
    ],
  };
}

function Tooltip({ tip }: { tip: Tip }) {
  return (
    <>
      <div
        style={{
          position: "absolute",
          left: tip.crossLeft,
          top: 0,
          width: 1,
          height: "100%",
          background: tip.crossColor,
          opacity: 0.55,
          pointerEvents: "none",
        }}
      />
      <div
        style={{
          position: "absolute",
          left: tip.left,
          top: tip.top,
          transform: tip.transform,
          pointerEvents: "none",
          background: "#05060a",
          border: "1px solid rgba(255,255,255,.13)",
          borderRadius: 9,
          padding: "8px 10px",
          boxShadow: "0 14px 34px rgba(0,0,0,.6)",
          zIndex: 20,
          whiteSpace: "nowrap",
        }}
      >
        <div
          style={{
            font: "700 10px/1 var(--font-manrope), sans-serif",
            letterSpacing: ".04em",
            textTransform: "uppercase",
            color: tip.titleColor,
            marginBottom: 7,
          }}
        >
          {tip.title}
        </div>
        {tip.rows.map((r, idx) => (
          <div
            key={idx}
            style={{
              display: "flex",
              justifyContent: "space-between",
              gap: 16,
              fontSize: 10.5,
              marginTop: 3,
            }}
          >
            <span style={{ color: "#6b7280" }}>{r.k}</span>
            <span className="mono" style={{ color: r.c }}>
              {r.v}
            </span>
          </div>
        ))}
      </div>
    </>
  );
}

const panelStyle: CSSProperties = { padding: "16px 18px" };

export interface RiskDashboardProps {
  selectedId: string | null;
  selected: PortfolioSummary | null;
  /** Bumped by the shell when a holding is added, to trigger a refetch. */
  refreshToken: number;
  onAddHolding: () => void;
}

export default function RiskDashboard({
  selectedId,
  selected,
  refreshToken,
  onAddHolding,
}: RiskDashboardProps) {
  const [payload, setPayload] = useState<RiskPayload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [conf, setConf] = useState<Confidence>(95);
  const [hover, setHover] = useState<HoverState | null>(null);

  const loadRisk = useCallback((id: string | null) => {
    if (!id) return Promise.resolve();
    return fetch(`/api/portfolio/${id}/risk`, { cache: "no-store" })
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.json();
      })
      .then((data: RiskPayload) => {
        setPayload(data);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    void loadRisk(selectedId);
  }, [selectedId, refreshToken, loadRisk]);

  const view = useMemo(
    () => (payload ? derive(payload, conf) : null),
    [payload, conf],
  );

  const mkMove =
    (chart: ChartKind, n: number) =>
    (e: React.MouseEvent<HTMLDivElement>) => {
      const r = e.currentTarget.getBoundingClientRect();
      const px = e.clientX - r.left;
      const py = e.clientY - r.top;
      let f = px / r.width;
      f = Math.max(0, Math.min(0.999999, f));
      const i = chart === "dist" ? Math.floor(f * n) : Math.round(f * (n - 1));
      setHover((H) =>
        !H || H.chart !== chart || H.i !== i || Math.abs(H.py - py) > 4
          ? { chart, i, px, py, w: r.width }
          : H,
      );
    };
  const onChartLeave = () => setHover(null);

  const segBase: CSSProperties = {
    padding: "5px 12px",
    borderRadius: 7,
    fontSize: 11.5,
    cursor: "pointer",
    height: "auto",
    minWidth: 0,
    lineHeight: 1,
  };
  const segOn: CSSProperties = {
    ...segBase,
    fontWeight: 700,
    background: "var(--accent)",
    color: "#fff",
  };
  const segOff: CSSProperties = {
    ...segBase,
    fontWeight: 600,
    background: "transparent",
    color: "#6b7280",
  };

  const controls = (
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
          Risk Analytics
        </div>
        <div className="pe-lbl" style={{ marginTop: 4 }}>
          {selected?.name ?? "—"} · Value-at-Risk &amp; stress
        </div>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
        {view && (
          <div className="mono" style={{ fontSize: 11.5, color: "#6b7280" }}>
            {view.asOf}
          </div>
        )}
        <div style={{ display: "flex", alignItems: "center", gap: 9 }}>
          <span className="pe-lbl">Confidence</span>
          <ToggleGroup
            value={[String(conf)]}
            onValueChange={(g) => {
              if (g[0]) setConf(Number(g[0]) as Confidence);
            }}
            style={{
              background: "#14161d",
              border: "1px solid rgba(255,255,255,.07)",
              borderRadius: 9,
              padding: 2,
              gap: 0,
            }}
          >
            <ToggleGroupItem value="95" style={conf === 95 ? segOn : segOff}>
              95%
            </ToggleGroupItem>
            <ToggleGroupItem value="99" style={conf === 99 ? segOn : segOff}>
              99%
            </ToggleGroupItem>
          </ToggleGroup>
        </div>
        <Button
          onClick={() => void loadRisk(selectedId)}
          className="h-auto"
          style={{
            gap: 8,
            padding: "8px 15px",
            borderRadius: 10,
            background: "var(--accent)",
            color: "#fff",
            fontWeight: 700,
            fontSize: 12.5,
            boxShadow: "0 6px 18px rgba(99,102,241,.35)",
            cursor: "pointer",
          }}
        >
          ▸ Run Analysis
        </Button>
      </div>
    </div>
  );

  if (!view) {
    return (
      <div style={{ flex: 1, padding: "18px 20px", minWidth: 0 }}>
        {controls}
        <div className="pe-panel" style={{ padding: 40, color: "#6b7280" }}>
          {error
            ? `Failed to load risk data: ${error}`
            : "Loading risk analytics…"}
        </div>
      </div>
    );
  }

  const distTip =
    hover?.chart === "dist" ? buildTip(view, conf, hover) : null;
  const histTip =
    hover?.chart === "hist" ? buildTip(view, conf, hover) : null;
  const ddTip = hover?.chart === "dd" ? buildTip(view, conf, hover) : null;

  return (
    <div style={{ flex: 1, padding: "18px 20px", minWidth: 0 }}>
      {controls}
            {payload?.positions.length === 0 && (
              <div
                style={{
                  marginBottom: 13,
                  padding: "10px 12px 10px 14px",
                  borderRadius: 10,
                  background: "rgba(99,102,241,.10)",
                  border: "1px solid rgba(99,102,241,.30)",
                  color: "#c5cad6",
                  fontSize: 12.5,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  gap: 12,
                }}
              >
                <span>
                  This book has no holdings yet — add one to see live analytics.
                </span>
                <Button
                  onClick={onAddHolding}
                  className="h-auto"
                  style={{
                    flexShrink: 0,
                    padding: "7px 13px",
                    borderRadius: 9,
                    background: "var(--accent)",
                    color: "#fff",
                    fontWeight: 700,
                    fontSize: 12,
                  }}
                >
                  ＋ Add Holding
                </Button>
              </div>
            )}
            {/* KPI row */}
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "repeat(7,1fr)",
                gap: 10,
              }}
            >
              {view.kpis.map((k) => (
                <Card
                  key={k.label}
                  className="pe-panel ring-0"
                  style={{ display: "block", padding: "12px 13px", gap: 0 }}
                >
                  <div className="pe-lbl" style={{ fontSize: 9.5 }}>
                    {k.label}
                  </div>
                  <div
                    className="mono"
                    style={{
                      fontSize: 18.5,
                      fontWeight: 600,
                      marginTop: 9,
                      letterSpacing: "-.01em",
                      whiteSpace: "nowrap",
                      color: k.valueColor,
                    }}
                  >
                    {k.value}
                  </div>
                  <div
                    style={{ fontSize: 10, marginTop: 6, color: k.subColor }}
                  >
                    {k.sub}
                  </div>
                </Card>
              ))}
            </div>

            {/* charts grid */}
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "1.62fr 1fr",
                gap: 13,
                marginTop: 13,
              }}
            >
              {/* left col */}
              <div
                style={{
                  display: "flex",
                  flexDirection: "column",
                  gap: 13,
                  minWidth: 0,
                }}
              >
                {/* P&L distribution */}
                <div className="pe-panel" style={panelStyle}>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "baseline",
                      justifyContent: "space-between",
                    }}
                  >
                    <div style={{ fontWeight: 700, fontSize: 13.5 }}>
                      Simulated 1-Day P&amp;L Distribution{" "}
                      <span style={{ color: "var(--accent)", fontWeight: 600 }}>
                        · {view.confL}
                      </span>
                    </div>
                    <div className="pe-lbl">Monte Carlo · 20,000 paths</div>
                  </div>
                  <div
                    style={{
                      position: "relative",
                      marginTop: 12,
                      cursor: "crosshair",
                    }}
                    onMouseMove={mkMove("dist", view.dist.binCount)}
                    onMouseLeave={onChartLeave}
                  >
                    <svg
                      viewBox="0 0 800 290"
                      preserveAspectRatio="none"
                      style={{
                        width: "100%",
                        height: 208,
                        display: "block",
                        overflow: "visible",
                      }}
                    >
                      {view.dist.bars.map((b, i) => (
                        <rect
                          key={i}
                          x={b.x}
                          y={b.y}
                          width={b.w}
                          height={b.h}
                          rx={1.5}
                          fill={b.fill}
                        />
                      ))}
                      <line
                        x1={view.dist.esx}
                        y1={2}
                        x2={view.dist.esx}
                        y2={274}
                        stroke="var(--loss)"
                        strokeWidth={1.8}
                        strokeDasharray="5 4"
                      />
                      <line
                        x1={view.dist.varx}
                        y1={2}
                        x2={view.dist.varx}
                        y2={274}
                        stroke="#fbbf24"
                        strokeWidth={1.8}
                        strokeDasharray="5 4"
                      />
                    </svg>
                    {distTip && <Tooltip tip={distTip} />}
                  </div>
                  <div
                    style={{
                      display: "flex",
                      gap: 18,
                      marginTop: 12,
                      flexWrap: "wrap",
                    }}
                  >
                    <div
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 7,
                        fontSize: 11,
                        color: "#9aa1b2",
                      }}
                    >
                      <span
                        style={{
                          width: 14,
                          height: 3,
                          borderRadius: 2,
                          background: "#fbbf24",
                        }}
                      />
                      VaR {view.confL}{" "}
                      <span className="mono" style={{ color: "#e8eaf0" }}>
                        {view.var1dBoth}
                      </span>
                    </div>
                    <div
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 7,
                        fontSize: 11,
                        color: "#9aa1b2",
                      }}
                    >
                      <span
                        style={{
                          width: 14,
                          height: 3,
                          borderRadius: 2,
                          background: "var(--loss)",
                        }}
                      />
                      ES {view.confL}{" "}
                      <span className="mono" style={{ color: "#e8eaf0" }}>
                        {view.es1dBoth}
                      </span>
                    </div>
                  </div>
                </div>

                {/* Drawdown */}
                <div className="pe-panel" style={panelStyle}>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "baseline",
                      justifyContent: "space-between",
                    }}
                  >
                    <div style={{ fontWeight: 700, fontSize: 13.5 }}>
                      Drawdown — Underwater Curve
                    </div>
                    <div style={{ fontSize: 11, color: "#9aa1b2" }}>
                      Max{" "}
                      <span className="mono" style={{ color: "var(--loss)" }}>
                        {view.dd.maxL}
                      </span>
                    </div>
                  </div>
                  <div
                    style={{
                      position: "relative",
                      marginTop: 12,
                      cursor: "crosshair",
                    }}
                    onMouseMove={mkMove("dd", view.dd.dates.length)}
                    onMouseLeave={onChartLeave}
                  >
                    <svg
                      viewBox="0 0 800 200"
                      preserveAspectRatio="none"
                      style={{ width: "100%", height: 150, display: "block" }}
                    >
                      <defs>
                        <linearGradient
                          id="ddgA"
                          x1="0"
                          y1="0"
                          x2="0"
                          y2="1"
                        >
                          <stop
                            offset="0%"
                            stopColor="var(--loss)"
                            stopOpacity={0.42}
                          />
                          <stop
                            offset="100%"
                            stopColor="var(--loss)"
                            stopOpacity={0.02}
                          />
                        </linearGradient>
                      </defs>
                      <line
                        x1={0}
                        y1={10}
                        x2={800}
                        y2={10}
                        stroke="rgba(255,255,255,.12)"
                        strokeWidth={1}
                        strokeDasharray="3 4"
                      />
                      <path d={view.dd.area} fill="url(#ddgA)" />
                      <path
                        d={view.dd.line}
                        fill="none"
                        stroke="var(--loss)"
                        strokeWidth={1.6}
                      />
                    </svg>
                    {ddTip && <Tooltip tip={ddTip} />}
                  </div>
                </div>
              </div>

              {/* right col */}
              <div
                style={{
                  display: "flex",
                  flexDirection: "column",
                  gap: 13,
                  minWidth: 0,
                }}
              >
                {/* Positions */}
                <div className="pe-panel" style={panelStyle}>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "baseline",
                      justifyContent: "space-between",
                      marginBottom: 4,
                    }}
                  >
                    <div style={{ fontWeight: 700, fontSize: 13.5 }}>
                      Positions
                    </div>
                    <div className="pe-lbl">
                      {view.positions.length} holdings
                    </div>
                  </div>
                  <Table style={{ tableLayout: "fixed" }}>
                    <colgroup>
                      <col style={{ width: "34.88%" }} />
                      <col style={{ width: "18.6%" }} />
                      <col style={{ width: "23.26%" }} />
                      <col style={{ width: "23.26%" }} />
                    </colgroup>
                    <TableHeader>
                      <TableRow
                        className="pe-lbl hover:bg-transparent"
                        style={{
                          borderBottom: "1px solid rgba(255,255,255,.06)",
                        }}
                      >
                        <TableHead
                          className="pe-lbl"
                          style={{ height: "auto", padding: "8px 0 7px" }}
                        >
                          Asset
                        </TableHead>
                        <TableHead
                          className="pe-lbl"
                          style={{
                            height: "auto",
                            padding: "8px 0 7px",
                            textAlign: "right",
                          }}
                        >
                          Last
                        </TableHead>
                        <TableHead
                          className="pe-lbl"
                          style={{
                            height: "auto",
                            padding: "8px 0 7px",
                            textAlign: "right",
                          }}
                        >
                          Mkt Val
                        </TableHead>
                        <TableHead
                          className="pe-lbl"
                          style={{
                            height: "auto",
                            padding: "8px 0 7px",
                            textAlign: "right",
                          }}
                        >
                          Unrl P&amp;L
                        </TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {view.positions.map((p) => (
                        <TableRow
                          key={p.t}
                          className="pe-row"
                          style={{
                            borderBottom: "1px solid rgba(255,255,255,.035)",
                          }}
                        >
                          <TableCell
                            style={{ padding: "8px 6px 8px 0", verticalAlign: "middle" }}
                          >
                            <div
                              style={{
                                display: "flex",
                                alignItems: "center",
                                gap: 6,
                              }}
                            >
                              <span
                                style={{ fontWeight: 700, fontSize: 12.5 }}
                              >
                                {p.t}
                              </span>
                              <Badge
                                variant="secondary"
                                className="mono"
                                style={{
                                  height: "auto",
                                  fontSize: 8.5,
                                  fontWeight: 400,
                                  padding: "1px 4px",
                                  borderRadius: 4,
                                  background: "rgba(255,255,255,.06)",
                                  color: "#7b8394",
                                }}
                              >
                                {p.ccy}
                              </Badge>
                            </div>
                            <div
                              style={{
                                fontSize: 10,
                                color: "#6b7280",
                                marginTop: 2,
                                whiteSpace: "nowrap",
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                maxWidth: 130,
                              }}
                            >
                              {p.name}
                            </div>
                            <div
                              style={{
                                height: 3,
                                borderRadius: 2,
                                background: "rgba(255,255,255,.07)",
                                marginTop: 5,
                                width: 96,
                              }}
                            >
                              <div
                                style={{
                                  height: "100%",
                                  borderRadius: 2,
                                  background: "var(--accent)",
                                  width: `${p.wtPct}%`,
                                }}
                              />
                            </div>
                          </TableCell>
                          <TableCell
                            className="mono"
                            style={{
                              padding: "8px 0",
                              textAlign: "right",
                              fontSize: 11.5,
                              color: "#c5cad6",
                              verticalAlign: "middle",
                            }}
                          >
                            {p.lastL}
                          </TableCell>
                          <TableCell
                            className="mono"
                            style={{
                              padding: "8px 0",
                              textAlign: "right",
                              fontSize: 11.5,
                              color: "#e8eaf0",
                              verticalAlign: "middle",
                            }}
                          >
                            {p.mvL}
                          </TableCell>
                          <TableCell
                            className="mono"
                            style={{
                              padding: "8px 0",
                              textAlign: "right",
                              fontSize: 11.5,
                              color: p.upnlColor,
                              verticalAlign: "middle",
                            }}
                          >
                            {p.upnlL}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>

                {/* Component VaR */}
                <div className="pe-panel" style={panelStyle}>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "baseline",
                      justifyContent: "space-between",
                      marginBottom: 11,
                    }}
                  >
                    <div style={{ fontWeight: 700, fontSize: 13.5 }}>
                      Component VaR
                    </div>
                    <div className="pe-lbl">{view.confL} · contribution</div>
                  </div>
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      gap: 9,
                    }}
                  >
                    {view.comps.map((c) => (
                      <div
                        key={c.t}
                        className="pe-comp-row"
                        style={{
                          display: "grid",
                          gridTemplateColumns: "46px 1fr 78px",
                          gap: 9,
                          alignItems: "center",
                          padding: "3px 6px",
                          margin: "0 -6px",
                          borderRadius: 7,
                        }}
                      >
                        <div
                          className="mono"
                          style={{ fontSize: 11.5, fontWeight: 600 }}
                        >
                          {c.t}
                        </div>
                        <div
                          style={{
                            height: 9,
                            borderRadius: 5,
                            background: "rgba(255,255,255,.05)",
                          }}
                        >
                          <div
                            style={{
                              height: "100%",
                              borderRadius: 5,
                              background:
                                "linear-gradient(90deg,var(--loss),var(--lossDeep))",
                              width: `${c.barPct}%`,
                            }}
                          />
                        </div>
                        <div
                          className="mono"
                          style={{
                            textAlign: "right",
                            fontSize: 11,
                            color: "#c5cad6",
                          }}
                        >
                          {c.valueL}{" "}
                          <span style={{ color: "#6b7280" }}>{c.pctL}</span>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </div>

            {/* Historical VaR */}
            <div className="pe-panel" style={{ ...panelStyle, marginTop: 13 }}>
              <div
                style={{
                  display: "flex",
                  alignItems: "baseline",
                  justifyContent: "space-between",
                }}
              >
                <div style={{ fontWeight: 700, fontSize: 13.5 }}>
                  Historical VaR — 1-Day &amp; 20-Day{" "}
                  <span style={{ color: "var(--accent)", fontWeight: 600 }}>
                    · {view.confL}
                  </span>
                </div>
                <div
                  style={{
                    display: "flex",
                    gap: 20,
                    alignItems: "baseline",
                  }}
                >
                  <div style={{ fontSize: 11, color: "#9aa1b2" }}>
                    1-Day now{" "}
                    <span className="mono" style={{ color: "var(--accent)" }}>
                      {view.histVar.cur1d}
                    </span>
                  </div>
                  <div style={{ fontSize: 11, color: "#9aa1b2" }}>
                    20-Day now{" "}
                    <span className="mono" style={{ color: "var(--loss)" }}>
                      {view.histVar.cur20d}
                    </span>
                  </div>
                </div>
              </div>
              <div
                style={{
                  position: "relative",
                  marginTop: 12,
                  cursor: "crosshair",
                }}
                onMouseMove={mkMove("hist", view.histVar.dates.length)}
                onMouseLeave={onChartLeave}
              >
                <svg
                  viewBox="0 0 800 200"
                  preserveAspectRatio="none"
                  style={{ width: "100%", height: 160, display: "block" }}
                >
                  <defs>
                    <linearGradient id="hvgA" x1="0" y1="0" x2="0" y2="1">
                      <stop
                        offset="0%"
                        stopColor="var(--accent)"
                        stopOpacity={0.26}
                      />
                      <stop
                        offset="100%"
                        stopColor="var(--accent)"
                        stopOpacity={0.02}
                      />
                    </linearGradient>
                  </defs>
                  <line
                    x1={0}
                    y1={185}
                    x2={800}
                    y2={185}
                    stroke="rgba(255,255,255,.10)"
                    strokeWidth={1}
                  />
                  <path d={view.histVar.area1d} fill="url(#hvgA)" />
                  <path
                    d={view.histVar.line20d}
                    fill="none"
                    stroke="var(--loss)"
                    strokeWidth={1.6}
                    strokeDasharray="5 4"
                  />
                  <path
                    d={view.histVar.line1d}
                    fill="none"
                    stroke="var(--accent)"
                    strokeWidth={2}
                  />
                </svg>
                {histTip && <Tooltip tip={histTip} />}
              </div>
              <div style={{ display: "flex", gap: 18, marginTop: 11 }}>
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 7,
                    fontSize: 11,
                    color: "#9aa1b2",
                  }}
                >
                  <span
                    style={{
                      width: 14,
                      height: 3,
                      borderRadius: 2,
                      background: "var(--accent)",
                    }}
                  />
                  VaR 1-Day
                </div>
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 7,
                    fontSize: 11,
                    color: "#9aa1b2",
                  }}
                >
                  <span
                    style={{
                      width: 14,
                      height: 3,
                      borderRadius: 2,
                      background: "var(--loss)",
                    }}
                  />
                  VaR 20-Day (√t scaled)
                </div>
                <div
                  style={{
                    fontSize: 11,
                    color: "#6b7280",
                    marginLeft: "auto",
                  }}
                >
                  180 trading days · 20d rolling window
                </div>
              </div>
            </div>
    </div>
  );
}
