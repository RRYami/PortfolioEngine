// Types for the "Portfolio Positions" page — mirrors the Rust
// `positions_view::PositionsPayload` contract (GET /api/portfolio/{id}/positions).

export interface Lot {
  date: string;
  quantity: number;
  /** Cost per unit at acquisition, native currency. */
  price: number;
  /** quantity * price, native currency. */
  cost: number;
}

export interface PositionDetail {
  ticker: string;
  name: string;
  ccy: string;
  quantity: number;
  /** Weighted-average cost per unit, native currency. */
  avgCost: number;
  /** Latest spot price, native currency. */
  last: number;
  /** Market value in base currency. */
  marketValue: number;
  /** Cost basis in base currency. */
  costBasis: number;
  weightPct: number;
  unrealizedPnl: number;
  unrealizedPnlPct: number;
  lots: Lot[];
}

export interface PositionsPayload {
  asOf: string;
  baseCcy: string;
  totalValue: number;
  positions: PositionDetail[];
}
