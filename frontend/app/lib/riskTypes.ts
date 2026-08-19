// Types for the Rust ↔ UI seam described in the design handoff README
// (design_handoff_risk_dashboard/README.md → "Data Contract").
//
// The engine supplies raw values for both confidence levels in one payload so
// the 95%/99% toggle can switch with no refetch. The dashboard does all
// *formatting* and *SVG geometry* from these numbers.

export type Confidence = 95 | 99;

export interface Position {
  ticker: string;
  name: string;
  ccy: string;
  qty: number;
  last: number;
  /** Market value in baseCcy (FX-converted). */
  marketValue: number;
  weightPct: number;
  unrealizedPnl: number;
}

export interface Drawdown {
  /** Most negative point of the underwater curve, in %. */
  maxPct: number;
  /** ISO dates, length N (≈180). */
  dates: string[];
  /** % from running peak, ≤ 0, length N. */
  series: number[];
}

export interface HistVar {
  /** ISO dates, length N. */
  dates: string[];
  /** Rolling 1-day VaR as a positive %, per confidence; each length N. */
  var1dPct: Record<Confidence, number[]>;
  /** Rolling 20-day VaR (= var1d × √20) as a positive %, per confidence. */
  var20dPct: Record<Confidence, number[]>;
}

export interface ComponentVar {
  ticker: string;
  /** Contribution to 1-day VaR (sums to var1d), base currency. */
  value: number;
  /** Whole-percent share of total VaR. */
  pctOfVar: number;
}

export interface RiskBlock {
  var1d: number;
  var1dPct: number;
  var20d: number;
  var20dPct: number;
  es1d: number;
  es1dPct: number;
  /** One per holding, sorted descending by contribution. */
  componentVar: ComponentVar[];
}

export interface Cutoffs {
  /** P&L value (negative) where the VaR cutoff line sits. */
  var: number;
  /** P&L value (negative) where the ES cutoff line sits. */
  es: number;
  /**
   * P&L value (negative) for the deep-tail colour boundary (deepP quantile).
   * Not part of the original two dashed lines, but supplied so the histogram
   * bars can be coloured central / in-tail / beyond-deep-tail exactly.
   */
  deep: number;
}

export interface PnlDistribution {
  paths: number;
  binLow: number;
  binHigh: number;
  binCount: number;
  /** binCount integers. */
  counts: number[];
  cutoffs: Record<Confidence, Cutoffs>;
}

export interface RiskPayload {
  asOf: string;
  baseCcy: string;
  portfolioValue: number;
  realizedPnl: number;
  unrealizedPnl: number;
  /** Today's move on the current book; null when not computable. */
  todayReturnPct: number | null;
  /** 252-day realized, annualized %. */
  annVolPct: number;
  positions: Position[];
  drawdown: Drawdown;
  histVar: HistVar;
  risk: Record<Confidence, RiskBlock>;
  pnlDistribution: PnlDistribution;
}
