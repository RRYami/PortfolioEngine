// Types for the "Portfolio Positions" page — mirrors the Rust
// `positions_view::PositionsPayload` contract (GET /api/portfolio/{id}/positions).

export interface Lot {
  date: string;
  quantity: number;
  /** Cost per unit at acquisition, native currency. */
  price: number;
  /** quantity * price, native currency. */
  cost: number;
  /** Native → base rate on this lot's trade date. */
  fxRate: number;
  /** `cost` converted at that trade-date rate. */
  costBase: number;
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
  /** Market value in base currency, at the spot rate. */
  marketValue: number;
  /** Market value in the position's own currency. */
  marketValueNative: number;
  /** Cost basis in base, each lot converted at its own trade-date rate. */
  costBasis: number;
  /** Cost basis in the position's own currency. */
  costBasisNative: number;
  weightPct: number;
  /** Total unrealized P&L in base currency (price + fx). */
  unrealizedPnl: number;
  /** Unrealized P&L in the position's own currency — the pure price move. */
  unrealizedPnlNative: number;
  /** The price move, expressed in base at today's rate. */
  unrealizedPnlPrice: number;
  /** What the base return gained or lost purely from the currency moving. */
  unrealizedPnlFx: number;
  unrealizedPnlPct: number;
  /** Spot rate used for market value, native → base. */
  fxRate: number;
  lots: Lot[];
}

export interface PositionsPayload {
  asOf: string;
  baseCcy: string;
  totalValue: number;
  positions: PositionDetail[];
}
