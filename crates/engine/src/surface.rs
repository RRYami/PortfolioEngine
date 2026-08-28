//! The volatility surface as the risk engine sees it: something that prices an
//! option today, and something that can be shocked by a handful of factors.
//!
//! Assembled from the offline artifacts — fitted SVI slices, the parity
//! forward curve, and the PCA of grid changes — so nothing here fits anything.
//! It reads a model that was already calibrated and validated.

use chrono::NaiveDate;

use crate::grid::{FittedSlice, sample};
use crate::ids::InstrumentId;
use crate::pca::PcaFit;
use crate::vol::{OptionRight, price};

/// A grid cell, matching the column order of [`SurfaceSnapshot::pca`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub z: f64,
    pub tte: f64,
}

/// One underlying's surface on one date, plus its factor model.
#[derive(Debug, Clone)]
pub struct SurfaceSnapshot {
    /// Parity forwards by maturity, ascending.
    pub forwards: Vec<(f64, f64)>,
    /// Continuously compounded rate behind the discount curve.
    pub rate: f64,
    /// Fitted smiles, ascending in maturity.
    pub slices: Vec<FittedSlice>,
    /// Grid cells, in the factor model's column order.
    pub cells: Vec<Cell>,
    pub pca: PcaFit,
    /// Session date of each row in `pca.scores`, ascending and the same length.
    ///
    /// The scores are a time series and have to be joined to spot returns by
    /// date, not by position: the surface is fitted only on sessions with a
    /// usable chain, so it skips days the price history keeps. Without these
    /// dates the two series get aligned on their tails, which slides the whole
    /// vol factor against spot and destroys the leverage effect.
    pub score_sessions: Vec<NaiveDate>,
}

impl SurfaceSnapshot {
    /// Forward at `tau`, linearly interpolated and flat outside the curve.
    ///
    /// Flat extrapolation rather than none: a position can outlive the listed
    /// chain, and refusing to price it would drop a real exposure from the
    /// report, which is worse than pricing it off the last observable forward.
    #[must_use]
    pub fn forward(&self, tau: f64) -> Option<f64> {
        let f = &self.forwards;
        let first = f.first()?;
        let last = f.last()?;
        if tau <= first.0 {
            return Some(first.1);
        }
        if tau >= last.0 {
            return Some(last.1);
        }
        let i = f.iter().position(|&(t, _)| t >= tau)?;
        let (t0, f0) = f[i - 1];
        let (t1, f1) = f[i];
        Some(f0 + (f1 - f0) * (tau - t0) / (t1 - t0))
    }

    #[must_use]
    pub fn discount(&self, tau: f64) -> f64 {
        (-self.rate * tau.max(0.0)).exp()
    }

    /// Base volatility at standardised moneyness `z` and maturity `tau`.
    #[must_use]
    pub fn vol(&self, z: f64, tau: f64) -> Option<f64> {
        sample(&self.slices, z, tau).map(|c| c.vol).or_else(|| {
            // Past the last fitted expiry, hold the terminal slice rather than
            // lose the position.
            let last = self.slices.last()?;
            Some(last.svi.vol(z * last.svi.total_variance(0.0).sqrt(), last.tte))
        })
    }

    /// Standardised moneyness of a strike at a given maturity.
    #[must_use]
    pub fn moneyness(&self, strike: f64, tau: f64) -> Option<f64> {
        let f = self.forward(tau)?;
        if !(strike > 0.0 && f > 0.0) {
            return None;
        }
        let w_atm = sample(&self.slices, 0.0, tau)
            .map(|c| c.total_variance)
            .or_else(|| self.slices.last().map(|s| s.svi.total_variance(0.0)))?;
        (w_atm > 0.0).then(|| (strike / f).ln() / w_atm.sqrt())
    }

    /// Multiplicative volatility shock at `(z, tau)` for a set of factor scores.
    ///
    /// The factor model lives on grid cells, but an option sits wherever its
    /// strike and expiry put it, so the reconstructed per-cell change is
    /// interpolated: linearly in `z` within each maturity row, then linearly
    /// across rows. Rows are used rather than a bilinear patch because the grid
    /// is not rectangular once cells that were mostly extrapolation are
    /// dropped — each row spans a different range of `z`.
    #[must_use]
    pub fn log_vol_shock(&self, z: f64, tau: f64, scores: &[f64]) -> f64 {
        let delta = self.pca.reconstruct(scores);
        // Group cell deltas by maturity, preserving cell order.
        let mut rows: Vec<(f64, Vec<(f64, f64)>)> = Vec::new();
        for (cell, d) in self.cells.iter().zip(delta.iter()) {
            match rows.iter_mut().find(|(t, _)| (t - cell.tte).abs() < 1e-9) {
                Some((_, pts)) => pts.push((cell.z, *d)),
                None => rows.push((cell.tte, vec![(cell.z, *d)])),
            }
        }
        if rows.is_empty() {
            return 0.0;
        }
        rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, pts) in &mut rows {
            pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        }
        let along = |pts: &[(f64, f64)]| -> f64 { interp(pts, z) };
        let by_tau: Vec<(f64, f64)> = rows.iter().map(|(t, p)| (*t, along(p))).collect();
        interp(&by_tau, tau)
    }

    /// Price one contract (not one lot) under an optional factor shock.
    ///
    /// `spot_ratio` scales the forward: a forward is spot times a carry factor,
    /// and over a `VaR` horizon the carry is a rounding error next to the spot
    /// move, so scaling is both simpler and more robust than rebuilding a
    /// carry curve per path.
    #[must_use]
    pub fn price_contract(
        &self,
        right: OptionRight,
        strike: f64,
        tau: f64,
        spot_ratio: f64,
        scores: &[f64],
    ) -> Option<f64> {
        if tau <= 0.0 {
            // At or past expiry the contract is worth its intrinsic value
            // against the shocked forward.
            let f = self.forward(0.0)? * spot_ratio;
            return Some(match right {
                OptionRight::Call => (f - strike).max(0.0),
                OptionRight::Put => (strike - f).max(0.0),
            });
        }
        let fwd = self.forward(tau)? * spot_ratio;
        let z = {
            let w_atm = sample(&self.slices, 0.0, tau)
                .map(|c| c.total_variance)
                .or_else(|| self.slices.last().map(|s| s.svi.total_variance(0.0)))?;
            (w_atm > 0.0).then(|| (strike / fwd).ln() / w_atm.sqrt())?
        };
        let base = self.vol(z, tau)?;
        let vol = if scores.is_empty() {
            base
        } else {
            base * self.log_vol_shock(z, tau, scores).exp()
        };
        Some(price(right, fwd, strike, tau, vol.max(1e-6), self.discount(tau)))
    }
}

/// Linear interpolation over sorted `(x, y)` points, flat outside the range.
fn interp(pts: &[(f64, f64)], x: f64) -> f64 {
    match (pts.first(), pts.last()) {
        (Some(f), Some(l)) => {
            if x <= f.0 {
                return f.1;
            }
            if x >= l.0 {
                return l.1;
            }
            for w in pts.windows(2) {
                if x <= w[1].0 {
                    let (x0, y0) = w[0];
                    let (x1, y1) = w[1];
                    return if (x1 - x0).abs() < f64::EPSILON {
                        y0
                    } else {
                        y0 + (y1 - y0) * (x - x0) / (x1 - x0)
                    };
                }
            }
            l.1
        }
        _ => 0.0,
    }
}

/// Where the risk engine gets a surface. Mirrors the other provider traits so
/// the engine keeps no knowledge of parquet, `DuckDB`, or any other source.
pub trait VolSurfaceProvider {
    fn surface(&self, underlying: InstrumentId, as_of: NaiveDate)
    -> Option<&SurfaceSnapshot>;
}

/// In-memory provider, keyed by underlying. Dates are ignored: a report is
/// computed as of one date, and the caller loads that date's surfaces.
#[derive(Debug, Default, Clone)]
pub struct StaticVolSurfaceProvider {
    surfaces: std::collections::HashMap<InstrumentId, SurfaceSnapshot>,
}

impl StaticVolSurfaceProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(mut self, underlying: InstrumentId, snapshot: SurfaceSnapshot) -> Self {
        self.surfaces.insert(underlying, snapshot);
        self
    }

    pub fn insert(&mut self, underlying: InstrumentId, snapshot: SurfaceSnapshot) {
        self.surfaces.insert(underlying, snapshot);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }
}

impl VolSurfaceProvider for StaticVolSurfaceProvider {
    fn surface(&self, underlying: InstrumentId, _as_of: NaiveDate) -> Option<&SurfaceSnapshot> {
        self.surfaces.get(&underlying)
    }
}
