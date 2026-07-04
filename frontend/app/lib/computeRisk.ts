// Server-side risk computation.
//
// This is the stand-in for the Rust analytics engine: it produces the raw
// numbers described in the Data Contract. The math (seeded Monte-Carlo VaR/ES,
// rolling-window historical VaR, underwater drawdown curve, component VaR) is
// ported verbatim from the design prototype's logic class so the rendered
// dashboard matches the standalone reference exactly. In production this module
// would call into the PortfolioEngine `Statistics`/VaR code instead.

import type {
  Confidence,
  RiskBlock,
  RiskPayload,
} from "./riskTypes";

const CONFIDENCES: Confidence[] = [95, 99];

// Deterministic PRNG (mulberry32) + Box–Muller, matching the prototype seed so
// the simulated distribution is reproducible.
function mulberry32(seed: number): () => number {
  let a = seed | 0;
  return function () {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface RawHolding {
  t: string;
  name: string;
  qty: number;
  avg: number;
  last: number;
  ccy: "USD" | "EUR";
  vol: number;
}

const FX: Record<string, number> = { USD: 1, EUR: 1.08 };

const RAW: RawHolding[] = [
  { t: "NVDA", name: "NVIDIA Corp", qty: 1500, avg: 64.5, last: 131.2, ccy: "USD", vol: 0.029 },
  { t: "AAPL", name: "Apple Inc", qty: 1200, avg: 142.3, last: 214.8, ccy: "USD", vol: 0.018 },
  { t: "MSFT", name: "Microsoft Corp", qty: 800, avg: 310, last: 448.2, ccy: "USD", vol: 0.017 },
  { t: "SPY", name: "S&P 500 ETF", qty: 900, avg: 410, last: 548, ccy: "USD", vol: 0.012 },
  { t: "AMZN", name: "Amazon.com Inc", qty: 600, avg: 128, last: 201.5, ccy: "USD", vol: 0.022 },
  { t: "ASML", name: "ASML Holding", qty: 220, avg: 580, last: 712, ccy: "EUR", vol: 0.024 },
  { t: "GLD", name: "SPDR Gold Trust", qty: 700, avg: 178, last: 218, ccy: "USD", vol: 0.01 },
  { t: "TLT", name: "20+Y Treasury ETF", qty: 1100, avg: 96, last: 88.4, ccy: "USD", vol: 0.009 },
];

/** Business-day ISO dates (YYYY-MM-DD), length n, ending on `end` inclusive. */
function businessDays(end: Date, n: number): string[] {
  const out: string[] = [];
  const cur = new Date(end);
  while (out.length < n) {
    const wd = cur.getDay();
    if (wd !== 0 && wd !== 6) {
      const y = cur.getFullYear();
      const m = String(cur.getMonth() + 1).padStart(2, "0");
      const d = String(cur.getDate()).padStart(2, "0");
      out.unshift(`${y}-${m}-${d}`);
    }
    cur.setDate(cur.getDate() - 1);
  }
  return out;
}

export function computeRiskPayload(_portfolioId: string): RiskPayload {
  const rnd = mulberry32(7);
  const randn = () => {
    let u = 0;
    let v = 0;
    while (!u) u = rnd();
    while (!v) v = rnd();
    return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v);
  };

  // Mark-to-market each holding.
  const holdings = RAW.map((p) => {
    const mv = p.qty * p.last * FX[p.ccy];
    const cost = p.qty * p.avg * FX[p.ccy];
    return { ...p, mv, cost, upnl: mv - cost };
  });
  const tot = holdings.reduce((a, p) => a + p.mv, 0);

  const wvol = holdings.reduce(
    (a, p) => a + (p.mv / tot) * p.vol,
    0,
  );
  const pdv = wvol * 0.84; // portfolio daily vol
  const annVol = pdv * Math.sqrt(252);

  // Monte-Carlo 1-day P&L (20,000 paths), sorted ascending.
  const Nsim = 20000;
  const pnl = new Array<number>(Nsim);
  for (let i = 0; i < Nsim; i++) pnl[i] = tot * (randn() * pdv + 0.0004);
  pnl.sort((a, b) => a - b);

  // Confidence-independent histogram, trimmed to the 0.4%–99.6% range.
  const qf = (p: number) => pnl[Math.floor(p * Nsim)];
  const binLow = qf(0.004);
  const binHigh = qf(0.996);
  const binCount = 43;
  const bw = (binHigh - binLow) / binCount;
  const counts = new Array<number>(binCount).fill(0);
  for (let i = 0; i < Nsim; i++) {
    const x = pnl[i];
    if (x < binLow || x > binHigh) continue;
    let bi = Math.floor((x - binLow) / bw);
    if (bi < 0) bi = 0;
    if (bi >= binCount) bi = binCount - 1;
    counts[bi]++;
  }

  // Equity-curve drawdown (underwater) over Nd trading days, with a stress
  // window injected to create a deep trough.
  const Nd = 180;
  const rs: number[] = [];
  for (let i = 0; i < Nd; i++) {
    let r = randn() * 0.0085 + 0.0007;
    if (i >= 96 && i <= 120) r = randn() * 0.011 - 0.0135;
    rs.push(r);
  }
  const eq = [1];
  for (let i = 1; i < Nd; i++) eq.push(eq[i - 1] * (1 + rs[i]));
  let peak = -1e9;
  const ddFrac = eq.map((e) => {
    peak = Math.max(peak, e);
    return e / peak - 1;
  });
  const maxDD = Math.min(...ddFrac);

  // Rolling 20-day realized vol of the daily returns.
  const Wv = 20;
  const rollSd: number[] = [];
  for (let t = 0; t < Nd; t++) {
    const s0 = Math.max(0, t - Wv);
    const win = rs.slice(s0, t + 1);
    const m = win.reduce((a, b) => a + b, 0) / win.length;
    let vc = 0;
    win.forEach((r) => (vc += (r - m) * (r - m)));
    rollSd.push(Math.sqrt(vc / Math.max(1, win.length - 1)));
  }
  const lastSd = rollSd[Nd - 1];

  const dates = businessDays(new Date(2026, 5, 25), Nd);

  // Component-VaR shares (confidence-independent fractions), sorted desc.
  const cs = holdings.map((p) => ({ t: p.t, c: p.mv * p.vol }));
  const csum = cs.reduce((a, b) => a + b.c, 0);
  const compShares = cs
    .map((o) => ({ t: o.t, share: o.c / csum }))
    .sort((a, b) => b.share - a.share);

  const sq20 = Math.sqrt(20);

  // Per-confidence headline metrics, cutoffs and historical series.
  const risk = {} as Record<Confidence, RiskBlock>;
  const cutoffs = {} as RiskPayload["pnlDistribution"]["cutoffs"];
  const histVar1d = {} as Record<Confidence, number[]>;
  const histVar20d = {} as Record<Confidence, number[]>;

  for (const conf of CONFIDENCES) {
    const tailP = conf === 99 ? 0.01 : 0.05;
    const deepP = conf === 99 ? 0.0025 : 0.01;
    const z = conf === 99 ? 2.326 : 1.645;

    const tV = pnl[Math.floor(tailP * Nsim)]; // negative P&L at the tail quantile
    const deepV = pnl[Math.floor(deepP * Nsim)];
    const var1d = -tV;

    const ec = Math.floor(tailP * Nsim);
    let esSum = 0;
    for (let i = 0; i < ec; i++) esSum += pnl[i];
    const esVal = esSum / ec; // negative
    const es1d = -esVal;
    const var20d = var1d * sq20;

    risk[conf] = {
      var1d,
      var1dPct: (var1d / tot) * 100,
      var20d,
      var20dPct: (var20d / tot) * 100,
      es1d,
      es1dPct: (es1d / tot) * 100,
      componentVar: compShares.map((o) => ({
        ticker: o.t,
        value: o.share * var1d,
        pctOfVar: Math.round(o.share * 100),
      })),
    };
    cutoffs[conf] = { var: tV, es: esVal, deep: deepV };

    // Rolling historical VaR, scaled so the latest 1-day point equals the
    // headline var1d fraction. (z cancels but is kept for clarity.)
    const targetFrac = var1d / tot;
    const scaleF = lastSd * z > 0 ? targetFrac / (lastSd * z) : 1;
    const hv1d = rollSd.map((sd) => sd * z * scaleF * 100); // positive %
    histVar1d[conf] = hv1d;
    histVar20d[conf] = hv1d.map((v) => v * sq20);
  }

  const positions = [...holdings]
    .sort((a, b) => b.mv - a.mv)
    .map((p) => ({
      ticker: p.t,
      name: p.name,
      ccy: p.ccy,
      qty: p.qty,
      last: p.last,
      marketValue: p.mv,
      weightPct: Number(((p.mv / tot) * 100).toFixed(1)),
      unrealizedPnl: p.upnl,
    }));

  const unrealizedPnl = holdings.reduce((a, p) => a + p.upnl, 0);

  return {
    asOf: "2026-06-25T16:00:00-04:00",
    baseCcy: "USD",
    portfolioValue: tot,
    realizedPnl: 184230.0,
    unrealizedPnl,
    todayReturnPct: 2.1,
    annVolPct: annVol * 100,
    positions,
    drawdown: {
      maxPct: maxDD * 100,
      dates,
      series: ddFrac.map((d) => d * 100),
    },
    histVar: {
      dates,
      var1dPct: histVar1d,
      var20dPct: histVar20d,
    },
    risk,
    pnlDistribution: {
      paths: Nsim,
      binLow,
      binHigh,
      binCount,
      counts,
      cutoffs,
    },
  };
}
