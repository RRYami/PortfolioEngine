// Pure derivation of the view-model: formatted strings + SVG chart geometry,
// computed from the engine payload and the selected confidence level. This is
// the "the dashboard does all formatting and SVG geometry" half of the seam —
// ported from the prototype's renderVals().

import type { Confidence, RiskPayload } from "./riskTypes";
import { asOfLabel, comp, MINUS, nf, shortDate } from "./format";

const ACCENT = "var(--accent)";
const GAIN = "var(--gain)";
const LOSS = "var(--loss)";
const LOSS_DEEP = "var(--lossDeep)";

export interface Kpi {
  label: string;
  value: string;
  valueColor: string;
  sub: string;
  subColor: string;
}

export interface PositionRow {
  t: string;
  name: string;
  ccy: string;
  lastL: string;
  mvL: string;
  wtPct: number;
  upnlL: string;
  upnlColor: string;
}

export interface CompRow {
  t: string;
  valueL: string;
  pctL: string;
  barPct: number;
}

export interface Bar {
  x: number;
  y: number;
  w: number;
  h: number;
  fill: string;
}

export interface DerivedView {
  confL: string;
  asOf: string;
  tot: number;
  kpis: Kpi[];
  positions: PositionRow[];
  comps: CompRow[];
  var1dBoth: string;
  es1dBoth: string;
  dist: {
    bars: Bar[];
    varx: number;
    esx: number;
    // raw values for hover tooltips
    binLow: number;
    binHigh: number;
    binCount: number;
    bw: number;
    counts: number[];
    paths: number;
    tV: number;
    deepV: number;
  };
  dd: {
    line: string;
    area: string;
    maxL: string;
    series: number[];
    dates: string[];
  };
  histVar: {
    line1d: string;
    area1d: string;
    line20d: string;
    cur1d: string;
    cur20d: string;
    v1d: number[];
    v20d: number[];
    dates: string[];
  };
}

const round1 = (n: number) => Number(n.toFixed(1));

export function derive(payload: RiskPayload, conf: Confidence): DerivedView {
  const tot = payload.portfolioValue;
  const confL = `${conf}%`;
  const r = payload.risk[conf];

  const pctS = (x: number) =>
    MINUS + (tot === 0 ? 0 : (x / tot) * 100).toFixed(2) + "%";
  const var1dPctS = pctS(r.var1d);
  const var20dPctS = pctS(r.var20d);
  const es1dPctS = pctS(r.es1d);

  // ---- KPI strip ----
  const kpis: Kpi[] = [
    {
      label: "Portfolio Value",
      value: comp(tot),
      valueColor: "#f5f7fb",
      sub: `▲ ${payload.todayReturnPct.toFixed(1)}% today`,
      subColor: GAIN,
    },
    {
      label: "VaR · 1-Day",
      value: comp(-r.var1d),
      valueColor: LOSS,
      sub: `${var1dPctS} · ${confL}`,
      subColor: LOSS,
    },
    {
      label: "VaR · 20-Day",
      value: comp(-r.var20d),
      valueColor: LOSS,
      sub: `${var20dPctS} · ${confL}`,
      subColor: LOSS,
    },
    {
      label: "Exp. Shortfall",
      value: comp(-r.es1d),
      valueColor: LOSS,
      sub: `${es1dPctS} · ${confL}`,
      subColor: LOSS,
    },
    {
      label: "Ann. Volatility",
      value: payload.annVolPct.toFixed(1) + "%",
      valueColor: "#f5f7fb",
      sub: "252-day realized",
      subColor: "#6b7280",
    },
    {
      label: "Max Drawdown",
      value: payload.drawdown.maxPct.toFixed(1) + "%",
      valueColor: LOSS,
      sub: "peak-to-trough",
      subColor: "#6b7280",
    },
    {
      label: "Unrealized P&L",
      value:
        (payload.unrealizedPnl >= 0 ? "+" : MINUS) +
        comp(Math.abs(payload.unrealizedPnl)),
      valueColor: payload.unrealizedPnl >= 0 ? GAIN : LOSS,
      sub: "on cost basis",
      subColor: "#6b7280",
    },
  ];

  // ---- Positions ----
  const positions: PositionRow[] = payload.positions.map((p) => ({
    t: p.ticker,
    name: p.name,
    ccy: p.ccy,
    lastL: (p.ccy === "EUR" ? "€" : "$") + nf(p.last, 2),
    mvL: comp(p.marketValue),
    wtPct: p.weightPct,
    upnlL:
      (p.unrealizedPnl >= 0 ? "+" : MINUS) +
      "$" +
      nf(Math.abs(p.unrealizedPnl), 0),
    upnlColor: p.unrealizedPnl >= 0 ? GAIN : LOSS,
  }));

  // ---- Component VaR ----
  const maxValue = r.componentVar[0]?.value ?? 1;
  const comps: CompRow[] = r.componentVar.map((c) => ({
    t: c.ticker,
    valueL: comp(c.value),
    pctL: c.pctOfVar + "%",
    barPct: round1((c.value / maxValue) * 100),
  }));

  // ---- P&L distribution histogram ----
  const { binLow: lo, binHigh: hi, binCount: nb, counts, paths } =
    payload.pnlDistribution;
  const bw = (hi - lo) / nb;
  const mc = Math.max(...counts, 1); // guard /0 when a book has no holdings
  const cut = payload.pnlDistribution.cutoffs[conf];
  const tV = cut.var;
  const deepV = cut.deep;
  const esVal = cut.es;
  const mapX = (x: number) => round1(((x - lo) / (hi - lo)) * 800);
  const bars: Bar[] = counts.map((c, i) => {
    const h = (c / mc) * 244;
    const x = mapX(lo + i * bw) + 1;
    const w = Math.max(1, 800 / nb - 2);
    const center = lo + (i + 0.5) * bw;
    let fill = ACCENT;
    if (center <= deepV) fill = LOSS_DEEP;
    else if (center <= tV) fill = LOSS;
    return {
      x,
      y: round1(268 - h),
      w: round1(w),
      h: round1(Math.max(0, h)),
      fill,
    };
  });

  const var1dBoth = comp(-r.var1d) + " (" + var1dPctS + ")";
  const es1dBoth = comp(-r.es1d) + " (" + es1dPctS + ")";

  // ---- Drawdown underwater curve ----
  const ddSeries = payload.drawdown.series; // % from peak, ≤ 0
  const maxDD = payload.drawdown.maxPct;
  const Nd = ddSeries.length;
  const ddPts = ddSeries.map((d, i) => {
    const x = round1((i / (Nd - 1)) * 800);
    const y = round1(10 + (maxDD !== 0 ? d / maxDD : 0) * 180);
    return [x, y] as const;
  });
  const ddLine = "M" + ddPts.map((p) => p[0] + " " + p[1]).join(" L ");
  const ddArea = ddLine + " L 800 10 L 0 10 Z";
  const ddDates = payload.drawdown.dates.map(shortDate);

  // ---- Historical VaR series ----
  const v1d = payload.histVar.var1dPct[conf]; // positive %
  const v20d = payload.histVar.var20dPct[conf];
  const maxHV = (Math.max(...v20d, 0) * 1.1) || 1; // guard empty / zero-vol
  const hMapY = (v: number) => round1(185 - (v / maxHV) * 173);
  const hX = (i: number) => round1((i / (Nd - 1)) * 800);
  const line1d = "M" + v1d.map((v, i) => hX(i) + " " + hMapY(v)).join(" L ");
  const area1d = line1d + " L 800 185 L 0 185 Z";
  const line20d = "M" + v20d.map((v, i) => hX(i) + " " + hMapY(v)).join(" L ");
  // `?? 0` so a book with no holdings (empty series) doesn't crash on .toFixed.
  const cur1dPct = v1d.at(-1) ?? 0;
  const cur20dPct = v20d.at(-1) ?? 0;
  const histDates = payload.histVar.dates.map(shortDate);

  return {
    confL,
    asOf: asOfLabel(payload.asOf),
    tot,
    kpis,
    positions,
    comps,
    var1dBoth,
    es1dBoth,
    dist: {
      bars,
      varx: mapX(tV),
      esx: mapX(esVal),
      binLow: lo,
      binHigh: hi,
      binCount: nb,
      bw,
      counts,
      paths,
      tV,
      deepV,
    },
    dd: {
      line: ddLine,
      area: ddArea,
      maxL: maxDD.toFixed(1) + "%",
      series: ddSeries,
      dates: ddDates,
    },
    histVar: {
      line1d,
      area1d,
      line20d,
      cur1d:
        MINUS + comp((cur1dPct / 100) * tot) + " · " + MINUS +
        cur1dPct.toFixed(2) + "%",
      cur20d:
        MINUS + comp((cur20dPct / 100) * tot) + " · " + MINUS +
        cur20dPct.toFixed(2) + "%",
      v1d,
      v20d,
      dates: histDates,
    },
  };
}
