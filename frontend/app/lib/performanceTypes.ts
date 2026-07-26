// Types for the "Performance" tab — mirrors the Rust
// `perf_view::PerformancePayload` contract
// (GET /api/portfolio/{id}/performance?rf=&benchmark=).

export interface RatioSet {
  sharpe: number;
  sortino: number;
  calmar: number;
  omega: number;
  annReturnPct: number;
  annVolPct: number;
  downsideDevPct: number;
  maxDrawdownPct: number;
  bestDayPct: number;
  worstDayPct: number;
  winRatePct: number;
  skew: number;
  kurtosis: number;
}

export interface Rolling {
  window: number;
  dates: string[];
  /** Aligned to `dates`; null until a full window is available. */
  sharpe: (number | null)[];
  sortino: (number | null)[];
}

export interface HorizonRow {
  label: string;
  days: number;
  sharpe: number;
  sortino: number;
  calmar: number;
  annReturnPct: number;
  annVolPct: number;
  maxDrawdownPct: number;
}

export interface RelativeStats {
  benchmark: string;
  beta: number;
  alphaPct: number;
  informationRatio: number;
  treynor: number;
  correlation: number;
  rSquared: number;
  upCapturePct: number;
  downCapturePct: number;
  benchmarkAnnReturnPct: number;
}

export interface PerformancePayload {
  asOf: string;
  baseCcy: string;
  rfAnnualPct: number;
  sampleDays: number;
  snapshot: RatioSet;
  rolling: Rolling;
  byHorizon: HorizonRow[];
  relative: RelativeStats | null;
}
