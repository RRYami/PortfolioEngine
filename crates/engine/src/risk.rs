use chrono::NaiveDate;
use rand::thread_rng;
use rand_distr::{Distribution, StandardNormal};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::prelude::ToPrimitive;

use crate::currency::Currency;
use crate::fx::FxRateProvider;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaRReport {
    pub as_of: NaiveDate,
    pub base_currency: Currency,
    pub entries: Vec<VaREntry>,
    pub per_asset: Vec<AssetRisk>,
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
pub fn compute_var(
    state: &PortfolioState,
    historical: &dyn HistoricalPriceProvider,
    fx: &dyn FxRateProvider,
    prices: &dyn crate::price::PriceProvider,
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
        });
    }

    // 1. Collect instruments, current prices, FX rates, and quantities.
    let assets: Vec<AssetInput> = gather_assets(state, prices, fx, base, as_of)?;

    // 2. Fetch history and compute log-returns.
    let returns_matrix = build_returns_matrix(&assets, historical, config.lookback_days, as_of)?;
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

    for sim in 0..config.num_simulations {
        let z: Vec<f64> = (0..n_assets)
            .map(|_| StandardNormal.sample(&mut rng))
            .collect();
        let lz = mat_vec_mul(&chol, &z);
        let simulated_returns: Vec<f64> = mean.iter().zip(lz.iter()).map(|(m, l)| m + l).collect();

        for (h_idx, h_scale) in horizons.iter().enumerate() {
            let mut portfolio_pnl = 0.0;
            for (a_idx, asset) in assets.iter().enumerate() {
                let ret = simulated_returns[a_idx] * h_scale;
                let p_current = asset.current_price_native.to_f64().unwrap_or(0.0);
                let p_sim = p_current * ret.exp();
                let qty = asset.quantity.to_f64().unwrap_or(0.0);
                let fx_rate = asset.fx_rate.to_f64().unwrap_or(1.0);
                let asset_pnl = qty * (p_current - p_sim) * fx_rate;
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

    Ok(VaRReport {
        as_of,
        base_currency: base,
        entries,
        per_asset,
    })
}

// ------------------------------------------------------------------
// Internal helpers
// ------------------------------------------------------------------

struct AssetInput {
    instrument: InstrumentId,
    symbol: String,
    quantity: Decimal,
    current_price_native: Decimal,
    current_value_base: Decimal,
    fx_rate: Decimal,
}

fn gather_assets(
    state: &PortfolioState,
    prices: &dyn crate::price::PriceProvider,
    fx: &dyn FxRateProvider,
    base: Currency,
    as_of: NaiveDate,
) -> Result<Vec<AssetInput>, RiskError> {
    let mut assets = Vec::new();
    for (inst_id, pos) in state.positions() {
        let price = prices.price(*inst_id, as_of)?;
        if price.currency != pos.currency() {
            return Err(RiskError::Price(
                crate::price::PriceError::PriceUnavailable {
                    instrument: *inst_id,
                    date: as_of,
                },
            ));
        }
        let fx_rate = fx.rate(pos.currency(), base, as_of)?;
        let qty = pos.net_quantity();
        let value_native = qty * price.amount;
        let value_base = value_native * fx_rate;

        // We need a symbol; callers should pass an Instrument slice or we
        // use the Display of InstrumentId as fallback.  For now we leave the
        // symbol resolution to the TUI layer (it already has the instrument
        // list).  We store an empty string here and the caller can patch it.
        assets.push(AssetInput {
            instrument: *inst_id,
            symbol: String::new(),
            quantity: qty,
            current_price_native: price.amount,
            current_value_base: value_base,
            fx_rate,
        });
    }
    Ok(assets)
}

fn build_returns_matrix(
    assets: &[AssetInput],
    historical: &dyn HistoricalPriceProvider,
    lookback: u32,
    as_of: NaiveDate,
) -> Result<Vec<Vec<f64>>, RiskError> {
    let from = as_of - chrono::Duration::days(i64::from(lookback));
    let mut matrix: Vec<Vec<f64>> = Vec::with_capacity(assets.len());

    for asset in assets {
        let series = historical.prices(asset.instrument, from, as_of)?;
        let need = lookback + 1;
        if series.len() < need as usize {
            return Err(RiskError::InsufficientHistory(
                asset.instrument,
                need,
                series.len(),
            ));
        }
        let mut returns = Vec::with_capacity(series.len() - 1);
        for window in series.windows(2) {
            let p_t = window[1].1.amount.to_f64().unwrap_or(1.0);
            let p_t_1 = window[0].1.amount.to_f64().unwrap_or(1.0);
            if p_t_1 <= 0.0 || p_t <= 0.0 {
                returns.push(0.0);
            } else {
                returns.push((p_t / p_t_1).ln());
            }
        }
        matrix.push(returns);
    }
    Ok(matrix)
}

#[allow(clippy::cast_precision_loss)]
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
            compute_var(&state, &hist, &fx, &prices, &config, Currency::USD, d(10)).unwrap();
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
            compute_var(&state, &hist, &fx, &prices, &config, Currency::USD, as_of).unwrap();

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
}
