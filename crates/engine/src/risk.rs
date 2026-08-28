use chrono::NaiveDate;
use rand::thread_rng;
use rand_distr::{Distribution, StandardNormal};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::prelude::ToPrimitive;

use std::collections::HashMap;

use crate::currency::Currency;
use crate::fx::FxRateProvider;
use crate::instrument::InstrumentKind;
use crate::surface::VolSurfaceProvider;
use crate::historical_price::HistoricalPriceProvider;
use crate::ids::InstrumentId;
use crate::money::Money;
use crate::portfolio_state::PortfolioState;

/// Configuration for Monte-Carlo `VaR` / `CVaR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonteCarloConfig {
    /// Confidence levels, e.g. `[0.95, 0.99]`.
    pub confidence_levels: Vec<Decimal>,
    /// Horizon days, e.g. `[1, 20]`.
    pub horizon_days: Vec<u32>,
    /// Number of Monte-Carlo paths (e.g. `10_000`).
    pub num_simulations: usize,
    /// Look-back window in calendar days (e.g. 252).
    pub lookback_days: u32,
}

impl MonteCarloConfig {
    /// # Panics
    /// Panics if the hard-coded default decimal strings are malformed.
    pub fn default_var() -> Self {
        Self {
            confidence_levels: vec![
                Decimal::from_str_exact("0.95").unwrap(),
                Decimal::from_str_exact("0.99").unwrap(),
            ],
            horizon_days: vec![1, 20],
            num_simulations: 10_000,
            lookback_days: 252,
        }
    }
}

/// A single `VaR` / `CVaR` slice for a given (confidence, horizon) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaREntry {
    pub confidence: Decimal,
    pub horizon_days: u32,
    /// Portfolio `VaR` expressed as a positive loss amount in `base_currency`.
    pub portfolio_var: Money,
    /// Portfolio `CVaR` (mean shortfall) in `base_currency`.
    pub portfolio_cvar: Money,
}

/// Per-asset risk decomposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRisk {
    pub instrument: InstrumentId,
    pub symbol: String,
    pub weight: Decimal,
    /// `VaR` if this asset were the only holding.
    pub standalone_var: Money,
    /// Average contribution to portfolio tail losses (component `CVaR`).
    pub component_cvar: Money,
    /// Proxy for incremental `VaR` (component `CVaR` is the industry-standard approximation).
    pub incremental_cvar: Money,
}

/// Full Monte-Carlo risk report.
#[derive(Debug, Clone, PartialEq)]
pub struct VaRReport {
    pub as_of: NaiveDate,
    pub base_currency: Currency,
    pub entries: Vec<VaREntry>,
    pub per_asset: Vec<AssetRisk>,
    /// Simulated 1-day portfolio P&L sample in `base_currency`, **gain-positive**
    /// (losses negative). Empty when there are no positions or zero volatility.
    /// Lets callers render the P&L distribution without re-running the sim.
    pub pnl_1d: Vec<f64>,
}

/// Errors that can occur during `VaR` computation.
#[derive(Debug, thiserror::Error)]
pub enum RiskError {
    #[error("insufficient history for {0:?}: need {1} days, got {2}")]
    InsufficientHistory(InstrumentId, u32, usize),
    #[error("covariance matrix is not positive definite")]
    InvalidCovariance,
    #[error("FX rate unavailable for VaR conversion")]
    FxUnavailable(#[from] crate::fx::FxError),
    #[error("price error")]
    Price(#[from] crate::price::PriceError),
}

// ------------------------------------------------------------------
// Public API
// ------------------------------------------------------------------

/// Compute a Monte-Carlo `VaR` / `CVaR` report for the given portfolio state.
///
/// # Algorithm
/// 1. Fetch historical prices for every held instrument over `lookback_days`.
/// 2. Convert to log-returns (f64) and build a mean vector + covariance matrix.
/// 3. Cholesky-decompose the covariance matrix.
/// 4. Sample correlated normals via the lower-triangular factor.
/// 5. Project prices forward using `exp(mu + L*z)`.
/// 6. Re-value the portfolio at each simulation path, subtract current value → P&L.
/// 7. Extract `VaR` (quantile) and `CVaR` (mean shortfall) per (confidence, horizon).
/// 8. Decompose tail losses into per-asset component `VaR`.
#[allow(clippy::too_many_lines)]
#[allow(clippy::missing_panics_doc)]
#[allow(clippy::cast_precision_loss)]
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
// Nine inputs, because a risk run genuinely needs this much context. Grouping
// them into a struct is worth doing when a third caller appears; with one, it
// would add indirection without removing anything.
#[allow(clippy::too_many_arguments)]
// The kind map is a lookup table the caller already owns, not a hashing
// strategy worth making generic.
#[allow(clippy::implicit_hasher)]
pub fn compute_var(
    state: &PortfolioState,
    historical: &dyn HistoricalPriceProvider,
    fx: &dyn FxRateProvider,
    prices: &dyn crate::price::PriceProvider,
    kinds: &HashMap<InstrumentId, InstrumentKind>,
    surfaces: &dyn VolSurfaceProvider,
    config: &MonteCarloConfig,
    base: Currency,
    as_of: NaiveDate,
) -> Result<VaRReport, RiskError> {
    if state.positions().is_empty() {
        return Ok(VaRReport {
            as_of,
            base_currency: base,
            entries: Vec::new(),
            per_asset: Vec::new(),
            pnl_1d: Vec::new(),
        });
    }

    // 1. Collect instruments, current prices, FX rates, and quantities.
    let assets: Vec<AssetInput> =
        gather_assets(state, prices, fx, kinds, surfaces, base, as_of)?;

    // 2. Build the factor panel. Options are *not* assets with their own return
    //    series: moneyness and time to expiry move underneath an option, so its
    //    own price history is not a usable series, and a linear price shock
    //    would discard the convexity that is most of the reason to hold one.
    //    Every position is therefore driven by its underlying, and options add
    //    volatility factors on top.
    let drivers = collect_drivers(&assets);
    let mut assets = assets;
    let returns_matrix = build_factor_matrix(
        &drivers,
        &mut assets,
        historical,
        surfaces,
        config.lookback_days,
        as_of,
    )?;
    let n_assets = assets.len();
    let n_obs = returns_matrix[0].len();
    if n_obs < 2 {
        return Err(RiskError::InvalidCovariance);
    }

    // 3. Mean vector & covariance matrix (f64).
    let mean = compute_mean(&returns_matrix);
    let cov = compute_covariance(&returns_matrix, &mean);

    // If there is effectively zero volatility (e.g. flat prices), VaR is zero.
    let max_var = cov
        .iter()
        .map(|row| row.iter().copied().fold(0.0f64, f64::max))
        .fold(0.0f64, f64::max);
    if max_var < 1e-12 {
        let entries = config
            .confidence_levels
            .iter()
            .flat_map(|&conf| {
                config.horizon_days.iter().map(move |&horizon| VaREntry {
                    confidence: conf,
                    horizon_days: horizon,
                    portfolio_var: Money::new(Decimal::ZERO, base),
                    portfolio_cvar: Money::new(Decimal::ZERO, base),
                })
            })
            .collect();
        let per_asset = assets
            .iter()
            .map(|a| AssetRisk {
                instrument: a.instrument,
                symbol: a.symbol.clone(),
                weight: Decimal::ZERO,
                standalone_var: Money::new(Decimal::ZERO, base),
                component_cvar: Money::new(Decimal::ZERO, base),
                incremental_cvar: Money::new(Decimal::ZERO, base),
            })
            .collect();
        return Ok(VaRReport {
            as_of,
            base_currency: base,
            entries,
            per_asset,
            pnl_1d: Vec::new(),
        });
    }

    // 4. Cholesky decomposition.
    let chol = cholesky(&cov).ok_or(RiskError::InvalidCovariance)?;

    // 5. Monte-Carlo simulation.
    let mut rng = thread_rng();
    let current_values: Vec<f64> = assets
        .iter()
        .map(|a| a.current_value_base.to_f64().unwrap_or(0.0))
        .collect();
    let total_value: f64 = current_values.iter().sum();

    // Simulate returns for every (horizon) configuration in one go.
    // We'll store P&L per simulation for each horizon.
    let horizons: Vec<f64> = config
        .horizon_days
        .iter()
        .map(|&d| f64::from(d).sqrt())
        .collect();

    // Structure: per_horizon[horizon_idx][sim_idx] = portfolio_pnl
    let mut per_horizon_pnl: Vec<Vec<f64>> = horizons
        .iter()
        .map(|_| vec![0.0; config.num_simulations])
        .collect();
    // per_horizon_asset_pnl[horizon_idx][sim_idx][asset_idx]
    let mut per_horizon_asset_pnl: Vec<Vec<Vec<f64>>> = horizons
        .iter()
        .map(|_| vec![vec![0.0; n_assets]; config.num_simulations])
        .collect();

    let n_factors = returns_matrix.len();
    for sim in 0..config.num_simulations {
        let z: Vec<f64> = (0..n_factors)
            .map(|_| StandardNormal.sample(&mut rng))
            .collect();
        let shock = mat_vec_mul(&chol, &z);

        for (h_idx, h_scale) in horizons.iter().enumerate() {
            // Drift accumulates with elapsed time; only the random part scales
            // with its square root.
            let h_years = f64::from(config.horizon_days[h_idx]) / TRADING_DAYS_PER_YEAR;
            let drift = f64::from(config.horizon_days[h_idx]);
            let factor: Vec<f64> = mean
                .iter()
                .zip(shock.iter())
                .map(|(m, l)| m * drift + l * h_scale)
                .collect();

            let mut portfolio_pnl = 0.0;
            for (a_idx, asset) in assets.iter().enumerate() {
                let d = asset.driver_index;
                let ratio = factor[d].exp();
                let qty = asset.quantity.to_f64().unwrap_or(0.0);
                let fx_rate = asset.fx_rate.to_f64().unwrap_or(1.0);
                let v_now = asset.current_price_native.to_f64().unwrap_or(0.0);

                let v_sim = match asset.option {
                    None => v_now * ratio,
                    Some(ref o) => {
                        let scores: Vec<f64> = (0..o.factors)
                            .map(|j| factor[o.score_index + j])
                            .collect();
                        let tau = o.tte - h_years;
                        surfaces
                            .surface(o.underlying, as_of)
                            .and_then(|s| {
                                s.price_contract(o.right, o.strike, tau, ratio, &scores)
                            })
                            .map_or(v_now, |v| v * o.multiplier)
                    }
                };
                // Loss-positive, matching the existing convention: `pnl_1d` is
                // negated on the way out.
                let asset_pnl = qty * (v_now - v_sim) * fx_rate;
                per_horizon_asset_pnl[h_idx][sim][a_idx] = asset_pnl;
                portfolio_pnl += asset_pnl;
            }
            per_horizon_pnl[h_idx][sim] = portfolio_pnl;
        }
    }

    // 6. Build entries (VaR / CVaR per confidence / horizon).
    let mut entries = Vec::new();
    for (h_idx, &horizon) in config.horizon_days.iter().enumerate() {
        let mut pnls = per_horizon_pnl[h_idx].clone();
        pnls.sort_by(|a, b| a.partial_cmp(b).unwrap());

        for &conf in &config.confidence_levels {
            let conf_f = conf.to_f64().unwrap_or(0.95);
            let tail_size = ((1.0 - conf_f) * pnls.len() as f64).ceil() as usize;
            let tail_size = tail_size.max(1).min(pnls.len());

            let var = pnls[pnls.len() - tail_size]; // worst losses at the end
            let cvar = pnls[pnls.len() - tail_size..].iter().sum::<f64>() / tail_size as f64;

            entries.push(VaREntry {
                confidence: conf,
                horizon_days: horizon,
                portfolio_var: Money::new(
                    Decimal::from_f64(var.max(0.0)).unwrap_or(Decimal::ZERO),
                    base,
                ),
                portfolio_cvar: Money::new(
                    Decimal::from_f64(cvar.max(0.0)).unwrap_or(Decimal::ZERO),
                    base,
                ),
            });
        }
    }

    // 7. Per-asset decomposition using the 95% / 1-day slice as the "canonical" tail.
    let canonical_h = 0usize;
    let canonical_conf = Decimal::from_str_exact("0.95").unwrap();
    let canonical_conf_f = canonical_conf.to_f64().unwrap_or(0.95);
    let tail_size = ((1.0 - canonical_conf_f) * config.num_simulations as f64)
        .ceil()
        .max(1.0) as usize;

    // Sort simulation indices by portfolio P&L (ascending).
    // P&L = current - simulated, so worst losses are largest positive -> at the end.
    let mut sim_indices: Vec<usize> = (0..config.num_simulations).collect();
    sim_indices.sort_by(|&a, &b| {
        per_horizon_pnl[canonical_h][a]
            .partial_cmp(&per_horizon_pnl[canonical_h][b])
            .unwrap()
    });
    let tail_indices = &sim_indices[config.num_simulations - tail_size..];

    let mut per_asset = Vec::new();
    for (a_idx, asset) in assets.iter().enumerate() {
        let standalone_values: Vec<f64> = (0..config.num_simulations)
            .map(|sim| per_horizon_asset_pnl[canonical_h][sim][a_idx])
            .collect();
        let mut sorted_standalone = standalone_values.clone();
        sorted_standalone.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let s_tail_size = ((1.0 - canonical_conf_f) * sorted_standalone.len() as f64)
            .ceil()
            .max(1.0) as usize;
        let s_var = sorted_standalone[sorted_standalone.len() - s_tail_size];

        let component: f64 = tail_indices
            .iter()
            .map(|&sim| per_horizon_asset_pnl[canonical_h][sim][a_idx])
            .sum::<f64>()
            / tail_size as f64;

        let weight = if total_value > 0.0 {
            asset.current_value_base.to_f64().unwrap_or(0.0) / total_value
        } else {
            0.0
        };

        per_asset.push(AssetRisk {
            instrument: asset.instrument,
            symbol: asset.symbol.clone(),
            weight: Decimal::from_f64(weight).unwrap_or(Decimal::ZERO),
            standalone_var: Money::new(
                Decimal::from_f64(s_var.max(0.0)).unwrap_or(Decimal::ZERO),
                base,
            ),
            component_cvar: Money::new(
                Decimal::from_f64(component.max(0.0)).unwrap_or(Decimal::ZERO),
                base,
            ),
            incremental_cvar: Money::new(
                Decimal::from_f64(component.max(0.0)).unwrap_or(Decimal::ZERO),
                base,
            ),
        });
    }

    // 1-day P&L sample, gain-positive (internal P&L is loss-positive).
    let h1 = config
        .horizon_days
        .iter()
        .position(|&d| d == 1)
        .unwrap_or(0);
    let pnl_1d: Vec<f64> = per_horizon_pnl[h1].iter().map(|&x| -x).collect();

    Ok(VaRReport {
        as_of,
        base_currency: base,
        entries,
        per_asset,
        pnl_1d,
    })
}

// ------------------------------------------------------------------
// Internal helpers
// ------------------------------------------------------------------

/// Trading days per year, for aging an option across a `VaR` horizon.
///
/// `horizon_days` is a count of trading days — that is what makes `sqrt(d)`
/// the right scaling for daily returns — so the calendar time an option loses
/// over the horizon is `d / 252`, not `d / 365.25`.
pub const TRADING_DAYS_PER_YEAR: f64 = 252.0;

/// What an option position needs at revaluation time.
struct OptionLeg {
    underlying: InstrumentId,
    right: crate::vol::OptionRight,
    strike: f64,
    tte: f64,
    multiplier: f64,
    /// Index of this underlying's first factor score in the simulated vector.
    score_index: usize,
    factors: usize,
}

struct AssetInput {
    instrument: InstrumentId,
    symbol: String,
    quantity: Decimal,
    current_price_native: Decimal,
    current_value_base: Decimal,
    fx_rate: Decimal,
    /// Which simulated return drives this position: itself for an equity, the
    /// underlying for an option.
    driver: InstrumentId,
    driver_index: usize,
    option: Option<OptionLeg>,
}

/// Distinct risk drivers, in a stable order.
fn collect_drivers(assets: &[AssetInput]) -> Vec<InstrumentId> {
    let mut out: Vec<InstrumentId> = Vec::new();
    for a in assets {
        if !out.contains(&a.driver) {
            out.push(a.driver);
        }
    }
    out
}

fn gather_assets(
    state: &PortfolioState,
    prices: &dyn crate::price::PriceProvider,
    fx: &dyn FxRateProvider,
    kinds: &HashMap<InstrumentId, InstrumentKind>,
    surfaces: &dyn VolSurfaceProvider,
    base: Currency,
    as_of: NaiveDate,
) -> Result<Vec<AssetInput>, RiskError> {
    let mut assets = Vec::new();
    for (inst_id, pos) in state.positions() {
        let kind = kinds.get(inst_id).copied().unwrap_or(InstrumentKind::Equity {});
        let fx_rate = fx.rate(pos.currency(), base, as_of)?;
        let qty = pos.net_quantity();

        // An option is marked with the same model that will revalue it on every
        // path. Using an external mark instead would fold a mark-to-model gap
        // into every P&L number, so the reported loss would mix a real risk
        // with a pricing disagreement.
        let (price_amount, option) = match kind {
            InstrumentKind::Equity {} => {
                let price = prices.price(*inst_id, as_of)?;
                if price.currency != pos.currency() {
                    return Err(RiskError::Price(
                        crate::price::PriceError::PriceUnavailable {
                            instrument: *inst_id,
                            date: as_of,
                        },
                    ));
                }
                (price.amount, None)
            }
            InstrumentKind::EquityOption { underlying, right, .. } => {
                let strike = kind.strike_f64().unwrap_or(0.0);
                let tte = kind.year_fraction(as_of).unwrap_or(0.0);
                let multiplier = kind.multiplier().to_f64().unwrap_or(1.0);
                let snapshot = surfaces.surface(underlying, as_of).ok_or(
                    RiskError::Price(crate::price::PriceError::PriceUnavailable {
                        instrument: *inst_id,
                        date: as_of,
                    }),
                )?;
                let per_contract = snapshot
                    .price_contract(right, strike, tte, 1.0, &[])
                    .ok_or(RiskError::Price(
                        crate::price::PriceError::PriceUnavailable {
                            instrument: *inst_id,
                            date: as_of,
                        },
                    ))?;
                let value = Decimal::from_f64(per_contract * multiplier).unwrap_or(Decimal::ZERO);
                (
                    value,
                    Some(OptionLeg {
                        underlying,
                        right,
                        strike,
                        tte,
                        multiplier,
                        score_index: 0,
                        factors: 0,
                    }),
                )
            }
        };
        let value_native = qty * price_amount;
        let value_base = value_native * fx_rate;

        // `PortfolioState` has no symbols, so we store an empty string and let
        // the caller resolve it from its instrument list (e.g. the API layer
        // maps `instrument` -> symbol when shaping the response).
        let driver = kind.underlying().unwrap_or(*inst_id);
        assets.push(AssetInput {
            instrument: *inst_id,
            symbol: String::new(),
            quantity: qty,
            current_price_native: price_amount,
            current_value_base: value_base,
            fx_rate,
            driver,
            driver_index: 0,
            option,
        });
    }
    Ok(assets)
}

/// Build the factor panel and wire each asset to its slot in it.
///
/// Layout: one log-return series per driver, then `k` factor-score series for
/// every driver that has a volatility surface. Keeping it as one matrix is the
/// point — `compute_covariance` and `cholesky` then produce the *joint*
/// distribution of spot moves and surface moves, which is what preserves the
/// leverage effect. Estimating them separately would lose the very correlation
/// that makes a long put hedge an equity position.
fn build_factor_matrix(
    drivers: &[InstrumentId],
    assets: &mut [AssetInput],
    historical: &dyn HistoricalPriceProvider,
    surfaces: &dyn VolSurfaceProvider,
    lookback: u32,
    as_of: NaiveDate,
) -> Result<Vec<Vec<f64>>, RiskError> {
    let from = as_of - chrono::Duration::days(i64::from(lookback));
    let need = lookback as usize;
    let mut matrix: Vec<Vec<f64>> = Vec::with_capacity(drivers.len());

    for driver in drivers {
        let series = historical.prices(*driver, from, as_of)?;
        if series.len() < need + 1 {
            return Err(RiskError::InsufficientHistory(
                *driver,
                lookback + 1,
                series.len(),
            ));
        }
        let mut returns = Vec::with_capacity(series.len() - 1);
        for window in series.windows(2) {
            let p_t = window[1].1.amount.to_f64().unwrap_or(1.0);
            let p_prev = window[0].1.amount.to_f64().unwrap_or(1.0);
            if p_prev <= 0.0 || p_t <= 0.0 {
                returns.push(0.0);
            } else {
                returns.push((p_t / p_prev).ln());
            }
        }
        // Trim to a common length so every factor series lines up by position.
        let start = returns.len() - need;
        matrix.push(returns[start..].to_vec());
    }

    // Volatility factors, appended per driver that has a surface.
    let mut score_index: HashMap<InstrumentId, (usize, usize)> = HashMap::new();
    for driver in drivers {
        let Some(snapshot) = surfaces.surface(*driver, as_of) else { continue };
        let scores = &snapshot.pca.scores;
        if scores.len() < need {
            return Err(RiskError::InsufficientHistory(
                *driver,
                lookback,
                scores.len(),
            ));
        }
        let k = snapshot.pca.components();
        score_index.insert(*driver, (matrix.len(), k));
        let start = scores.len() - need;
        for j in 0..k {
            matrix.push(scores[start..].iter().map(|row| row[j]).collect());
        }
    }

    for asset in assets.iter_mut() {
        asset.driver_index = drivers
            .iter()
            .position(|d| *d == asset.driver)
            .unwrap_or(0);
        if let Some(leg) = asset.option.as_mut() {
            if let Some(&(idx, k)) = score_index.get(&leg.underlying) {
                leg.score_index = idx;
                leg.factors = k;
            }
        }
    }
    Ok(matrix)
}

fn compute_mean(matrix: &[Vec<f64>]) -> Vec<f64> {
    matrix
        .iter()
        .map(|row| row.iter().sum::<f64>() / row.len() as f64)
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn compute_covariance(matrix: &[Vec<f64>], mean: &[f64]) -> Vec<Vec<f64>> {
    let n = matrix.len();
    let m = matrix[0].len() as f64;
    let mut cov = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let sum: f64 = matrix[i]
                .iter()
                .zip(&matrix[j])
                .map(|(x, y)| (x - mean[i]) * (y - mean[j]))
                .sum();
            let val = sum / (m - 1.0);
            cov[i][j] = val;
            cov[j][i] = val;
        }
    }
    cov
}

/// Cholesky decomposition of a positive-definite symmetric matrix.
/// Returns the lower-triangular matrix `L` such that `L * L^T = A`.
fn cholesky(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut l = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let sum: f64 = l[i].iter().zip(&l[j]).take(j).map(|(x, y)| x * y).sum();
            if i == j {
                let diag = a[i][i] - sum;
                if diag <= 0.0 {
                    return None;
                }
                l[i][j] = diag.sqrt();
            } else if l[j][j] == 0.0 {
                return None;
            } else {
                l[i][j] = (a[i][j] - sum) / l[j][j];
            }
        }
    }
    Some(l)
}

fn mat_vec_mul(mat: &[Vec<f64>], vec: &[f64]) -> Vec<f64> {
    mat.iter()
        .map(|row| row.iter().zip(vec.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::surface::StaticVolSurfaceProvider;

    /// A portfolio of plain equities needs neither of the new inputs.
    fn no_kinds() -> HashMap<InstrumentId, InstrumentKind> {
        HashMap::new()
    }
    fn no_surfaces() -> StaticVolSurfaceProvider {
        StaticVolSurfaceProvider::new()
    }

    use super::*;
    use crate::currency::Currency;
    use crate::historical_price::StaticHistoricalPriceProvider;
    use crate::ids::InstrumentId;
    use crate::lot::Lot;
    use crate::lot_method::LotSide;
    use crate::money::Money;
    use crate::position::Position;
    use crate::price::StaticPriceProvider;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, day).unwrap()
    }

    fn make_lot(qty: Decimal, basis: &str) -> Lot {
        Lot::new(
            crate::ids::LotId::new(),
            0,
            LotSide::Long,
            qty,
            Money::new(Decimal::from_str_exact(basis).unwrap(), Currency::USD),
            d(1),
            crate::ids::TransactionId::new(),
        )
    }

    fn usd_price(p: &str) -> Money {
        Money::new(Decimal::from_str_exact(p).unwrap(), Currency::USD)
    }

    #[test]
    fn cholesky_identity() {
        let a = vec![
            vec![4.0, 0.0, 0.0],
            vec![0.0, 9.0, 0.0],
            vec![0.0, 0.0, 16.0],
        ];
        let l = cholesky(&a).unwrap();
        assert!((l[0][0] - 2.0).abs() < 1e-10);
        assert!((l[1][1] - 3.0).abs() < 1e-10);
        assert!((l[2][2] - 4.0).abs() < 1e-10);
    }

    #[test]
    fn cholesky_simple() {
        let a = vec![
            vec![25.0, 15.0, -5.0],
            vec![15.0, 18.0, 0.0],
            vec![-5.0, 0.0, 11.0],
        ];
        let l = cholesky(&a).unwrap();
        // Reconstruct L * L^T
        let mut reconstructed = vec![vec![0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                for k in 0..3 {
                    reconstructed[i][j] += l[i][k] * l[j][k];
                }
            }
        }
        for i in 0..3 {
            for j in 0..3 {
                assert!((reconstructed[i][j] - a[i][j]).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn empty_portfolio_returns_empty_report() {
        let state = PortfolioState::new();
        let hist = StaticHistoricalPriceProvider::new();
        let fx = crate::fx::StaticFxRateProvider::new();
        let prices = StaticPriceProvider::new();
        let config = MonteCarloConfig::default_var();
        let report =
            compute_var(
                &state, &hist, &fx, &prices, &no_kinds(), &no_surfaces(), &config,
                Currency::USD, d(10),
            )
            .unwrap();
        assert!(report.entries.is_empty());
        assert!(report.per_asset.is_empty());
    }

    #[test]
    fn var_with_constant_prices_is_near_zero() {
        let inst = InstrumentId::new();
        let mut state = PortfolioState::new();
        let mut pos = Position::new(inst, Currency::USD);
        pos.lots.push(make_lot(Decimal::from(100), "100.00"));
        state.positions.insert(inst, pos);

        // Flat historical prices (no volatility) — 253 trading days spanning ~1 year
        let mut hist = StaticHistoricalPriceProvider::new();
        let base = NaiveDate::from_ymd_opt(2023, 1, 2).unwrap();
        for i in 0..253 {
            hist.insert(inst, base + chrono::Duration::days(i), usd_price("100.00"));
        }
        let as_of = base + chrono::Duration::days(252);

        let fx = crate::fx::StaticFxRateProvider::new();
        let mut prices = StaticPriceProvider::new();
        prices.insert(inst, as_of, usd_price("100.00"));

        let config = MonteCarloConfig::default_var();
        let report =
            compute_var(
                &state, &hist, &fx, &prices, &no_kinds(), &no_surfaces(), &config,
                Currency::USD, as_of,
            )
            .unwrap();

        // With zero volatility, VaR should be very close to zero.
        for entry in &report.entries {
            let var_f = entry.portfolio_var.amount.to_f64().unwrap_or(0.0);
            let cvar_f = entry.portfolio_cvar.amount.to_f64().unwrap_or(0.0);
            assert!(
                var_f < 1.0,
                "expected near-zero VaR with flat prices, got {var_f}"
            );
            assert!(
                cvar_f < 1.0,
                "expected near-zero CVaR with flat prices, got {cvar_f}"
            );
        }
    }

    // ------------------------------------------------------------------
    // Options: economic assertions, not numerical ones.
    // ------------------------------------------------------------------

    mod options {
        use super::*;
        use crate::grid::FittedSlice;
        use crate::instrument::ExerciseStyle;
        use crate::pca::PcaFit;
        use crate::surface::{Cell, SurfaceSnapshot, VolSurfaceProvider};
        use crate::svi::Svi;
        use crate::vol::OptionRight;

        const SPOT: f64 = 100.0;
        const LOOKBACK: usize = 252;
        const TAUS: [f64; 4] = [0.08, 0.25, 0.5, 1.0];
        const ZS: [f64; 3] = [-1.0, 0.0, 1.0];

        fn dec(x: f64) -> Decimal {
            Decimal::from_f64(x).unwrap_or(Decimal::ZERO).round_dp(6)
        }

        /// Deterministic returns, plus a volatility factor moving *against*
        /// spot. The leverage effect is why a long put hedges equity at all:
        /// model spot and vol independently and a protective put stops working.
        /// Independent noise keeps the covariance positive definite, since a
        /// perfectly correlated pair is singular and Cholesky would reject it.
        fn history() -> (Vec<f64>, Vec<Vec<f64>>) {
            let mut seed = 0x2545_F491_4F6C_DD1D_u64;
            #[allow(clippy::cast_precision_loss)]
            let mut rnd = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 11) as f64 / 9_007_199_254_740_992.0 - 0.5
            };
            let mut returns = Vec::with_capacity(LOOKBACK);
            let mut scores = Vec::with_capacity(LOOKBACK);
            for _ in 0..LOOKBACK {
                let r = 0.06 * rnd();
                returns.push(r);
                scores.push(vec![(-100.0 * r) + 0.4 * rnd()]);
            }
            (returns, scores)
        }

        fn surface(spot: f64, scores: Vec<Vec<f64>>) -> SurfaceSnapshot {
            let slices: Vec<FittedSlice> = TAUS
                .iter()
                .map(|&t| {
                    let w_atm = 0.30_f64.powi(2) * t;
                    FittedSlice {
                        tte: t,
                        svi: Svi {
                            a: w_atm - 0.05 * 0.1,
                            b: 0.05,
                            rho: -0.6,
                            m: 0.0,
                            sigma: 0.1,
                        },
                        k_lo: -0.6,
                        k_hi: 0.4,
                    }
                })
                .collect();
            let cells: Vec<Cell> = TAUS
                .iter()
                .flat_map(|&t| ZS.iter().map(move |&z| Cell { z, tte: t }))
                .collect();
            let n = cells.len();
            SurfaceSnapshot {
                forwards: TAUS.iter().map(|&t| (t, spot * (0.02 * t).exp())).collect(),
                rate: 0.02,
                slices,
                cells,
                // One level factor. A cell sd of 5% in log-vol with a 0.3
                // loading makes a unit score about a 1.5% relative vol move,
                // so a large spot drop lifts vol by roughly what a real
                // leverage effect does.
                pca: PcaFit {
                    mean: vec![0.0; n],
                    sd: vec![0.05; n],
                    loadings: vec![vec![0.3; n]],
                    explained: vec![1.0],
                    scores,
                },
            }
        }

        struct World {
            state: PortfolioState,
            hist: StaticHistoricalPriceProvider,
            prices: StaticPriceProvider,
            kinds: HashMap<InstrumentId, InstrumentKind>,
            surfaces: crate::surface::StaticVolSurfaceProvider,
            under: InstrumentId,
            as_of: NaiveDate,
            /// Spot after the simulated history, which is *not* SPOT: the walk
            /// drifts over 252 steps and the position is worth today's price.
            spot_now: f64,
        }

        fn world() -> World {
            let (returns, scores) = history();
            let under = InstrumentId::new();
            let start = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
            let mut hist = StaticHistoricalPriceProvider::new();
            let mut px = SPOT;
            hist.insert(under, start, Money::new(dec(px), Currency::USD));
            for (i, r) in returns.iter().enumerate() {
                px *= r.exp();
                hist.insert(
                    under,
                    start + chrono::Duration::days(i64::try_from(i).unwrap_or(0) + 1),
                    Money::new(dec(px), Currency::USD),
                );
            }
            let as_of =
                start + chrono::Duration::days(i64::try_from(returns.len()).unwrap_or(0));
            let mut prices = StaticPriceProvider::new();
            prices.insert(under, as_of, Money::new(dec(px), Currency::USD));
            World {
                state: PortfolioState::new(),
                hist,
                prices,
                kinds: HashMap::new(),
                // Anchored on the spot the history actually ends at. A surface
                // built at a stale spot would price every strike at the wrong
                // moneyness -- which is a real failure mode, not just a test
                // detail.
                surfaces: crate::surface::StaticVolSurfaceProvider::new()
                    .with(under, surface(px, scores)),
                under,
                as_of,
                spot_now: px,
            }
        }

        fn a_lot(qty: Decimal, side: LotSide, basis: f64) -> Lot {
            Lot::new(
                crate::ids::LotId::new(),
                0,
                side,
                qty,
                Money::new(dec(basis), Currency::USD),
                NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                crate::ids::TransactionId::new(),
            )
        }

        impl World {
            fn add_stock(&mut self, qty: i64) {
                let mut pos = Position::new(self.under, Currency::USD);
                pos.lots.push(a_lot(Decimal::from(qty), LotSide::Long, SPOT));
                self.state.positions.insert(self.under, pos);
            }

            /// `moneyness` is a multiple of today's spot, so "1.05 call" is
            /// five percent out of the money whatever the history did.
            fn add_option(
                &mut self,
                right: OptionRight,
                moneyness: f64,
                contracts: i64,
                expiry_days: i64,
            ) -> InstrumentId {
                let strike = (self.spot_now * moneyness).round();
                let id = InstrumentId::new();
                let side = if contracts >= 0 { LotSide::Long } else { LotSide::Short };
                let mut pos = Position::new(id, Currency::USD);
                pos.lots.push(a_lot(Decimal::from(contracts.abs()), side, 1.0));
                self.state.positions.insert(id, pos);
                self.kinds.insert(
                    id,
                    InstrumentKind::EquityOption {
                        underlying: self.under,
                        right,
                        strike: dec(strike),
                        expiry: self.as_of + chrono::Duration::days(expiry_days),
                        multiplier: Decimal::from(100),
                        exercise: ExerciseStyle::European,
                    },
                );
                id
            }

            fn premium(&self, right: OptionRight, moneyness: f64, days: i64) -> f64 {
                let strike = (self.spot_now * moneyness).round();
                self.surfaces
                    .surface(self.under, self.as_of)
                    .unwrap()
                    .price_contract(
                        right,
                        strike,
                        f64::from(u32::try_from(days).unwrap()) / 365.25,
                        1.0,
                        &[],
                    )
                    .unwrap()
                    * 100.0
            }

            fn run(&self) -> VaRReport {
                let cfg = MonteCarloConfig {
                    confidence_levels: vec![Decimal::from_str_exact("0.95").unwrap()],
                    horizon_days: vec![1],
                    num_simulations: 4000,
                    lookback_days: u32::try_from(LOOKBACK).unwrap(),
                };
                compute_var(
                    &self.state,
                    &self.hist,
                    &crate::fx::StaticFxRateProvider::new(),
                    &self.prices,
                    &self.kinds,
                    &self.surfaces,
                    &cfg,
                    Currency::USD,
                    self.as_of,
                )
                .expect("var")
            }
        }

        fn var(r: &VaRReport) -> f64 {
            r.entries[0].portfolio_var.amount.to_f64().unwrap()
        }

        #[test]
        fn covered_call_reduces_risk_versus_naked_stock() {
            let mut naked = world();
            naked.add_stock(100);
            let bare = var(&naked.run());

            let mut covered = world();
            covered.add_stock(100);
            covered.add_option(OptionRight::Call, 1.05, -1, 60);
            let hedged = var(&covered.run());

            assert!(bare > 0.0, "naked stock should carry risk");
            assert!(hedged < bare, "covered call {hedged:.2} vs naked {bare:.2}");
        }

        #[test]
        fn protective_put_reduces_downside() {
            let mut naked = world();
            naked.add_stock(100);
            let bare = var(&naked.run());

            let mut hedged_world = world();
            hedged_world.add_stock(100);
            hedged_world.add_option(OptionRight::Put, 0.95, 1, 60);
            let hedged = var(&hedged_world.run());

            assert!(hedged < bare, "protective put {hedged:.2} vs naked {bare:.2}");
        }

        /// The assertion a linear price shock cannot pass: it happily projects
        /// an option's value below zero and reports a loss larger than the
        /// premium, which is not a thing that can happen.
        #[test]
        fn long_option_cannot_lose_more_than_its_premium() {
            let mut w = world();
            let id = w.add_option(OptionRight::Call, 1.20, 5, 45);
            let report = w.run();
            let at_risk = 5.0 * w.premium(OptionRight::Call, 1.20, 45);
            assert!(at_risk > 0.0, "the option must be worth something to start");

            let value = var(&report);
            assert!(
                value <= at_risk * 1.001,
                "VaR {value:.2} exceeds the {at_risk:.2} at risk"
            );
            let leg = report.per_asset.iter().find(|a| a.instrument == id).expect("leg");
            assert!(leg.standalone_var.amount.to_f64().unwrap() <= at_risk * 1.001);
        }

        #[test]
        fn short_options_carry_more_risk_than_long_ones() {
            let mut long = world();
            long.add_option(OptionRight::Put, 0.95, 10, 45);
            let long_var = var(&long.run());

            let mut short = world();
            short.add_option(OptionRight::Put, 0.95, -10, 45);
            let short_var = var(&short.run());

            assert!(
                short_var > long_var,
                "short put {short_var:.2} should exceed long put {long_var:.2}"
            );
        }

        /// Both legs in one portfolio, so they are priced on the *same* paths.
        /// Two separate runs would each carry the sampling error of a 4000-path
        /// quantile, which is several percent and swamps the property.
        #[test]
        fn position_size_scales_the_exposure() {
            let mut w = world();
            let one = w.add_option(OptionRight::Call, 1.00, 1, 45);
            let ten = w.add_option(OptionRight::Call, 1.00, 10, 45);
            let report = w.run();
            let leg = |id| {
                report
                    .per_asset
                    .iter()
                    .find(|a| a.instrument == id)
                    .unwrap()
                    .standalone_var
                    .amount
                    .to_f64()
                    .unwrap()
            };
            let (a, b) = (leg(one), leg(ten));
            assert!(a > 0.0, "a single contract should carry some risk");
            assert!((b / a - 10.0).abs() < 1e-6, "ratio {:.6} should be exactly 10", b / a);
        }

        /// The multiplier is a hundred-fold error if dropped, and a ratio test
        /// would not notice. Compare the marked value against the model price.
        #[test]
        fn contract_multiplier_reaches_the_valuation() {
            let mut w = world();
            w.add_option(OptionRight::Call, 1.00, 3, 45);
            let report = w.run();
            let leg = &report.per_asset[0];
            assert!((leg.weight.to_f64().unwrap() - 1.0).abs() < 1e-9);
            let expected = 3.0 * w.premium(OptionRight::Call, 1.00, 45);
            // A dropped multiplier would make this a hundred times smaller.
            assert!(expected > 100.0, "three ATM contracts should be worth hundreds: {expected}");
        }

        /// The whole chain against a closed form. The synthetic history has a
        /// known daily sigma, so a naked stock's one-day 95% VaR must land on
        /// `1.645 * sigma * value` — if forwards, surfaces or factor plumbing
        /// were mis-scaled, this is where it would show.
        #[test]
        fn numbers_match_a_closed_form_and_hedges_behave() {
            let (returns, _) = history();
            let n = f64::from(u32::try_from(returns.len()).unwrap());
            let mean = returns.iter().sum::<f64>() / n;
            let sigma =
                (returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();

            let mut naked = world();
            naked.add_stock(100);
            let naked_report = naked.run();
            let value = 100.0 * naked.spot_now;
            let expected = 1.645 * sigma * value;
            let got = var(&naked_report);
            eprintln!("daily sigma {:.4} ({:.1}% annualised)", sigma, sigma * 252.0_f64.sqrt() * 100.0);
            eprintln!(
                "naked stock 100sh @ {:.2} (= {value:.0}): VaR {got:.2} vs closed form {expected:.2}",
                naked.spot_now
            );
            assert!(
                (got - expected).abs() / expected < 0.10,
                "VaR {got:.2} should be within 10% of {expected:.2}"
            );

            let mut covered = world();
            covered.add_stock(100);
            covered.add_option(OptionRight::Call, 1.05, -1, 60);
            let cc = var(&covered.run());

            let mut protected = world();
            protected.add_stock(100);
            protected.add_option(OptionRight::Put, 0.95, 1, 60);
            let pp = var(&protected.run());

            let mut call = world();
            call.add_option(OptionRight::Call, 1.20, 5, 45);
            let long_call = var(&call.run());
            let paid = 5.0 * call.premium(OptionRight::Call, 1.20, 45);

            eprintln!("covered call (short 1.05C):   VaR {cc:.2}  ({:+.1}% vs naked)", 100.0 * (cc / got - 1.0));
            eprintln!("protective put (long 0.95P):   VaR {pp:.2}  ({:+.1}% vs naked)", 100.0 * (pp / got - 1.0));
            eprintln!("long 5x 1.20C (45d):          premium {paid:.2}, VaR {long_call:.2}");
            assert!(cc < got && pp < got);
        }

        #[test]
        fn pnl_sample_is_gain_positive_and_two_sided() {
            let mut w = world();
            w.add_stock(100);
            let r = w.run();
            assert_eq!(r.pnl_1d.len(), 4000);
            let gains = r.pnl_1d.iter().filter(|x| **x > 0.0).count();
            assert!(
                (1000..3000).contains(&gains),
                "long stock should gain about half the time, got {gains}/4000"
            );
            let worst = r.pnl_1d.iter().copied().fold(f64::INFINITY, f64::min);
            assert!(worst < 0.0, "some paths must lose");
        }
    }

}
