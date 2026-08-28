// Pure derivation of the view-model: the formatted strings and the raw series
// the charts need, computed from the engine payload and the selected
// confidence level.
//
// Chart *geometry* deliberately lives no longer here — d3 builds scales and
// paths at render time from these raw numbers, so this module only formats.

import type { Confidence, RiskPayload } from "./riskTypes";
import { asOfLabel, ccySym, comp, MINUS, nf } from "./format";

const GAIN = "var(--gain)";
const LOSS = "var(--loss)";

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
  /** Contributes negatively — a hedge, offsetting risk rather than adding it. */
  hedge: boolean;
}

export interface DerivedView {
  /** Portfolio base currency — every converted figure is rendered in it. */
  baseCcy: string;
  confL: string;
  asOf: string;
  tot: number;
  kpis: Kpi[];
  positions: PositionRow[];
  comps: CompRow[];
  var1dBoth: string;
  es1dBoth: string;
  dist: {
    binLow: number;
    binHigh: number;
    binCount: number;
    /** Bin width, in P&L currency units. */
    bw: number;
    counts: number[];
    paths: number;
    /** P&L cutoffs (negative): VaR line, ES line, deep-tail colour boundary. */
    varV: number;
    esV: number;
    deepV: number;
  };
  dd: {
    maxL: string;
    series: number[];
    /** ISO dates — the time scale parses these; tooltips format them. */
    dates: string[];
  };
  histVar: {
    cur1d: string;
    cur20d: string;
    v1d: number[];
    v20d: number[];
    /** ISO dates. */
    dates: string[];
  };
}

const round1 = (n: number) => Number(n.toFixed(1));

export function derive(payload: RiskPayload, conf: Confidence): DerivedView {
  const tot = payload.portfolioValue;
  const bc = payload.baseCcy;
  const confL = `${conf}%`;
  const today = payload.todayReturnPct;
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
      value: comp(tot, bc),
      valueColor: "#f5f7fb",
      // Arrow and colour follow the sign; a null (nothing to compare against)
      // shows a dash rather than a flat 0.0%, which would read as a real move.
      sub:
        today == null
          ? "— today"
          : `${today >= 0 ? "▲" : "▼"} ${nf(Math.abs(today), 1)}% today`,
      subColor: today == null ? "#6b7280" : today >= 0 ? GAIN : LOSS,
    },
    {
      label: "VaR · 1-Day",
      value: comp(-r.var1d, bc),
      valueColor: LOSS,
      sub: `${var1dPctS} · ${confL}`,
      subColor: LOSS,
    },
    {
      label: "VaR · 20-Day",
      value: comp(-r.var20d, bc),
      valueColor: LOSS,
      sub: `${var20dPctS} · ${confL}`,
      subColor: LOSS,
    },
    {
      label: "Exp. Shortfall",
      value: comp(-r.es1d, bc),
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
        comp(Math.abs(payload.unrealizedPnl), bc),
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
    // `last` is a native-currency quote; `unrealizedPnl` is already in base.
    lastL: ccySym(p.ccy) + nf(p.last, 2),
    mvL: comp(p.marketValue, bc),
    wtPct: p.weightPct,
    upnlL:
      (p.unrealizedPnl >= 0 ? "+" : MINUS) +
      ccySym(bc) +
      nf(Math.abs(p.unrealizedPnl), 0),
    upnlColor: p.unrealizedPnl >= 0 ? GAIN : LOSS,
  }));

  // ---- Component VaR ----
  // Scaled on the largest *magnitude*, because a contribution can be negative:
  // a hedge makes money on the paths where the book loses most, so it takes
  // risk away. Bars are drawn by magnitude and coloured by sign; a negative
  // width is not a width at all.
  const maxAbs =
    r.componentVar.reduce((m, c) => Math.max(m, Math.abs(c.value)), 0) || 1;
  const comps: CompRow[] = r.componentVar.map((c) => ({
    t: c.ticker,
    valueL: comp(c.value, bc),
    pctL: c.pctOfVar + "%",
    barPct: round1((Math.abs(c.value) / maxAbs) * 100),
    hedge: c.value < 0,
  }));

  // ---- P&L distribution histogram ----
  const { binLow: lo, binHigh: hi, binCount: nb, counts, paths } =
    payload.pnlDistribution;
  const bw = (hi - lo) / nb;
  const cut = payload.pnlDistribution.cutoffs[conf];

  const var1dBoth = comp(-r.var1d, bc) + " (" + var1dPctS + ")";
  const es1dBoth = comp(-r.es1d, bc) + " (" + es1dPctS + ")";

  // ---- Drawdown underwater curve ----
  const ddSeries = payload.drawdown.series; // % from peak, ≤ 0
  const maxDD = payload.drawdown.maxPct;

  // ---- Historical VaR series ----
  const v1d = payload.histVar.var1dPct[conf]; // positive %
  const v20d = payload.histVar.var20dPct[conf];
  // `?? 0` so a book with no holdings (empty series) doesn't crash on .toFixed.
  const cur1dPct = v1d.at(-1) ?? 0;
  const cur20dPct = v20d.at(-1) ?? 0;

  return {
    baseCcy: bc,
    confL,
    asOf: asOfLabel(payload.asOf),
    tot,
    kpis,
    positions,
    comps,
    var1dBoth,
    es1dBoth,
    dist: {
      binLow: lo,
      binHigh: hi,
      binCount: nb,
      bw,
      counts,
      paths,
      varV: cut.var,
      esV: cut.es,
      deepV: cut.deep,
    },
    dd: {
      maxL: maxDD.toFixed(1) + "%",
      series: ddSeries,
      dates: payload.drawdown.dates,
    },
    histVar: {
      cur1d:
        MINUS + comp((cur1dPct / 100) * tot, bc) + " · " + MINUS +
        cur1dPct.toFixed(2) + "%",
      cur20d:
        MINUS + comp((cur20dPct / 100) * tot, bc) + " · " + MINUS +
        cur20dPct.toFixed(2) + "%",
      v1d,
      v20d,
      dates: payload.histVar.dates,
    },
  };
}
