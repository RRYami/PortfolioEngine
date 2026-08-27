//! Principal components of daily surface changes.
//!
//! The panel is one row per session and one column per grid cell, holding
//! *log*-volatility changes. Log rather than additive because a reconstructed
//! shock has to stay positive: a 15-vol cell with a 3-point daily sigma is only
//! a five-sigma move from a negative implied volatility, and the 99% tail of a
//! twenty-day horizon reaches there routinely.
//!
//! Every column is standardised before decomposing. Measured on the SOXX grid,
//! per-cell daily sd spans 0.016 to 0.114 — a factor of seven — so a raw
//! covariance decomposition is dominated by the one-month column and its first
//! component is not a level factor at all, merely "short-dated vol is noisy".
//! No variance is lost: the scale lives in [`PcaFit::sd`] and is restored by
//! [`PcaFit::reconstruct`].

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PcaError {
    TooFewObservations { got: usize, need: usize },
    /// A column never moved, so it cannot be standardised and carries no
    /// information. Upstream should have dropped it.
    ConstantColumn(usize),
    /// Panel had no columns, or ragged rows.
    Malformed,
    /// Fewer components requested than the panel can supply.
    TooManyComponents { asked: usize, cells: usize },
}

impl fmt::Display for PcaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewObservations { got, need } => {
                write!(f, "{got} observations, need at least {need}")
            }
            Self::ConstantColumn(i) => write!(f, "column {i} has zero variance"),
            Self::Malformed => write!(f, "panel is empty or ragged"),
            Self::TooManyComponents { asked, cells } => {
                write!(f, "asked for {asked} components from {cells} cells")
            }
        }
    }
}

impl std::error::Error for PcaError {}

/// Fewest daily changes worth decomposing. Below roughly twice the cell count
/// the covariance structure is not estimable and the loadings are noise.
pub const MIN_OBSERVATIONS: usize = 40;

/// A fitted decomposition, carrying everything needed to reconstruct a shock.
#[derive(Debug, Clone, PartialEq)]
pub struct PcaFit {
    /// Per-cell mean of the change over the fit window.
    pub mean: Vec<f64>,
    /// Per-cell standard deviation — the scale standardisation removed.
    pub sd: Vec<f64>,
    /// `k` loading vectors, each one value per cell.
    pub loadings: Vec<Vec<f64>>,
    /// Fraction of variance per component, for *all* components, descending.
    pub explained: Vec<f64>,
    /// Historical score per observation, `k` values each. Stored as a series
    /// rather than a covariance so the caller can estimate the joint
    /// distribution of scores and spot returns with its own machinery.
    pub scores: Vec<Vec<f64>>,
}

impl PcaFit {
    #[must_use]
    pub fn cells(&self) -> usize {
        self.mean.len()
    }

    #[must_use]
    pub fn components(&self) -> usize {
        self.loadings.len()
    }

    /// Turn factor scores into a per-cell log-vol change.
    ///
    /// Inverts the standardisation on the way out, which is the step that puts
    /// the variance back: a score of 1 on the first component means one
    /// standard deviation *of that component*, and each cell responds by its
    /// own loading times its own sd.
    #[must_use]
    pub fn reconstruct(&self, scores: &[f64]) -> Vec<f64> {
        let mut out = self.mean.clone();
        for (j, s) in scores.iter().enumerate().take(self.loadings.len()) {
            for (i, l) in self.loadings[j].iter().enumerate() {
                out[i] += self.sd[i] * s * l;
            }
        }
        out
    }

    /// Apply a reconstructed change to a surface, in vol space.
    ///
    /// Exponentiating is what keeps the shocked surface positive however far
    /// into the tail the scores go.
    #[must_use]
    pub fn shock(&self, base_vols: &[f64], scores: &[f64]) -> Vec<f64> {
        let d = self.reconstruct(scores);
        base_vols
            .iter()
            .zip(d.iter())
            .map(|(v, dv)| v * dv.exp())
            .collect()
    }

    /// Cumulative variance explained by the retained components.
    #[must_use]
    pub fn retained_variance(&self) -> f64 {
        self.explained.iter().take(self.loadings.len()).sum()
    }
}

/// Decompose a panel of daily changes into `k` components.
///
/// `panel` is row-major: one row per observation, one column per grid cell.
pub fn fit(panel: &[Vec<f64>], k: usize) -> Result<PcaFit, PcaError> {
    let rows = panel.len();
    if rows < MIN_OBSERVATIONS {
        return Err(PcaError::TooFewObservations { got: rows, need: MIN_OBSERVATIONS });
    }
    let n = panel.first().map_or(0, Vec::len);
    if n == 0 || panel.iter().any(|r| r.len() != n) {
        return Err(PcaError::Malformed);
    }
    if k == 0 || k > n {
        return Err(PcaError::TooManyComponents { asked: k, cells: n });
    }

    // Column-major from here: every Jacobi rotation touches two whole columns.
    let mut cols: Vec<Vec<f64>> = (0..n).map(|j| panel.iter().map(|r| r[j]).collect()).collect();
    #[allow(clippy::cast_precision_loss)] // panels are thousands of rows, not 2^53
    let denom = rows as f64;
    let mut mean = vec![0.0; n];
    let mut sd = vec![0.0; n];
    for (j, col) in cols.iter_mut().enumerate() {
        let m = col.iter().sum::<f64>() / denom;
        // Sample standard deviation: the mean was estimated from this same
        // window, so the divisor is rows-1.
        let var = col.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (denom - 1.0);
        let dev = var.sqrt();
        if dev <= 0.0 || !dev.is_finite() {
            return Err(PcaError::ConstantColumn(j));
        }
        for x in col.iter_mut() {
            *x = (*x - m) / dev;
        }
        mean[j] = m;
        sd[j] = dev;
    }

    let (mut sv, mut v) = jacobi_svd(&mut cols);

    // Order by singular value, descending. Jacobi leaves them arbitrary.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| sv[b].partial_cmp(&sv[a]).unwrap_or(std::cmp::Ordering::Equal));
    sv = order.iter().map(|&i| sv[i]).collect();
    v = order.iter().map(|&i| v[i].clone()).collect();
    let scores_cols: Vec<Vec<f64>> = order.iter().map(|&i| cols[i].clone()).collect();
    let mut scores_cols = scores_cols;

    // Pin the sign. Eigenvectors are sign-ambiguous, so an unpinned refit flips
    // a component for no reason and silently inverts every score already
    // stored against it. Convention: each component's largest-magnitude
    // loading is positive.
    for (loading, score) in v.iter_mut().zip(scores_cols.iter_mut()) {
        let flip = loading
            .iter()
            .max_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap_or(std::cmp::Ordering::Equal))
            .is_some_and(|&x| x < 0.0);
        if flip {
            for x in loading.iter_mut().chain(score.iter_mut()) {
                *x = -*x;
            }
        }
    }

    let total: f64 = sv.iter().map(|s| s * s).sum();
    let explained: Vec<f64> =
        if total > 0.0 { sv.iter().map(|s| s * s / total).collect() } else { vec![0.0; n] };

    let loadings: Vec<Vec<f64>> = v.into_iter().take(k).collect();
    let scores: Vec<Vec<f64>> =
        (0..rows).map(|i| (0..k).map(|j| scores_cols[j][i]).collect()).collect();

    Ok(PcaFit { mean, sd, loadings, explained, scores })
}

/// One-sided Jacobi SVD of a column-major matrix.
///
/// Chosen over forming the Gram matrix and eigendecomposing it, which is
/// faster but squares the condition number — and with one component carrying
/// half the variance these columns are strongly correlated to begin with. At
/// the sizes involved the difference is unmeasurable: 20 cells against five
/// years of sessions runs in about 11 ms, because the cost is driven by the
/// cell count, not the history length.
///
/// Returns the singular values and the right singular vectors as columns;
/// `cols` is left holding `U*Sigma`, whose columns are the scores.
fn jacobi_svd(cols: &mut [Vec<f64>]) -> (Vec<f64>, Vec<Vec<f64>>) {
    const MAX_SWEEPS: usize = 60;
    const TOL: f64 = 1e-14;
    /// Rotate a disjoint pair of columns in place.
    fn rotate(a: &mut [f64], b: &mut [f64], cosine: f64, sine: f64) {
        for (x, y) in a.iter_mut().zip(b.iter_mut()) {
            let (u, w) = (*x, *y);
            *x = cosine.mul_add(u, -(sine * w));
            *y = sine.mul_add(u, cosine * w);
        }
    }

    let n = cols.len();
    let mut v: Vec<Vec<f64>> = (0..n)
        .map(|j| (0..n).map(|i| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();

    for _ in 0..MAX_SWEEPS {
        let mut worst = 0.0_f64;
        for p in 0..n {
            for q in (p + 1)..n {
                let (mut alpha, mut beta, mut gamma) = (0.0, 0.0, 0.0);
                for (x, y) in cols[p].iter().zip(cols[q].iter()) {
                    alpha += x * x;
                    beta += y * y;
                    gamma += x * y;
                }
                let scale = (alpha * beta).sqrt();
                if scale <= 0.0 || gamma.abs() <= TOL * scale {
                    continue;
                }
                worst = worst.max(gamma.abs() / scale);
                // The rotation that annihilates the pair's inner product.
                let zeta = (beta - alpha) / (2.0 * gamma);
                let tangent = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let cosine = 1.0 / (1.0 + tangent * tangent).sqrt();
                let sine = cosine * tangent;
                // p < q always, so splitting at q yields the two disjoint
                // mutable columns the rotation needs.
                let (left, right) = cols.split_at_mut(q);
                rotate(&mut left[p], &mut right[0], cosine, sine);
                let (lv, rv) = v.split_at_mut(q);
                rotate(&mut lv[p], &mut rv[0], cosine, sine);
            }
        }
        if worst < TOL {
            break;
        }
    }
    let sv = cols.iter().map(|c| c.iter().map(|x| x * x).sum::<f64>().sqrt()).collect();
    (sv, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic panel built from `k` known factors plus per-cell noise,
    /// with each cell given a deliberately different scale so the
    /// standardisation is actually load-bearing.
    fn synthetic(rows: usize, cells: usize, noise: f64) -> Vec<Vec<f64>> {
        // xorshift64, so the panel is identical on every run.
        let mut seed = 987_654_321_u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            #[allow(clippy::cast_precision_loss)]
            let u = (seed >> 11) as f64 / 9_007_199_254_740_992.0;
            u - 0.5
        };
        (0..rows)
            .map(|_| {
                let (f1, f2) = (rnd(), rnd());
                (0..cells)
                    .map(|j| {
                        let (jf, cf) = (f64::from(u32::try_from(j).unwrap()),
                                        f64::from(u32::try_from(cells).unwrap()));
                        let tilt = jf / cf - 0.5;
                        // Scale rises across the grid, mimicking the 7x sd
                        // spread the real panel has.
                        let scale = 6.0f64.mul_add(jf / cf, 1.0);
                        scale * (0.9 * f1 + 0.6 * tilt * f2 + noise * rnd())
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn recovers_a_two_factor_structure() {
        let p = synthetic(400, 12, 0.05);
        let fit = fit(&p, 3).expect("fit");
        assert_eq!(fit.cells(), 12);
        assert_eq!(fit.components(), 3);
        // Two real factors plus small noise: the first two must dominate.
        assert!(fit.explained[0] > 0.6, "PC1 {}", fit.explained[0]);
        assert!(fit.explained[0] + fit.explained[1] > 0.95, "PC1+2 {:?}", &fit.explained[..2]);
        // PC1 is the common factor, so every cell loads the same way.
        assert!(fit.loadings[0].iter().all(|&l| l > 0.0), "PC1 not uniform: {:?}", fit.loadings[0]);
    }

    #[test]
    fn explained_variance_is_a_descending_distribution() {
        let fit = fit(&synthetic(300, 10, 0.3), 3).expect("fit");
        let total: f64 = fit.explained.iter().sum();
        assert!((total - 1.0).abs() < 1e-9, "sums to {total}");
        for w in fit.explained.windows(2) {
            assert!(w[0] >= w[1] - 1e-12, "not descending: {:?}", fit.explained);
        }
        assert!(fit.retained_variance() <= 1.0 + 1e-12);
    }

    #[test]
    fn signs_are_pinned_so_refits_agree() {
        let p = synthetic(300, 10, 0.2);
        let a = fit(&p, 3).expect("fit");
        // Negating the whole panel is the classic way an unpinned
        // decomposition flips its components; the convention must absorb it.
        let flipped: Vec<Vec<f64>> =
            p.iter().map(|r| r.iter().map(|x| -x).collect()).collect();
        let b = fit(&flipped, 3).expect("fit");
        for j in 0..3 {
            let big = a.loadings[j]
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.abs().partial_cmp(&y.1.abs()).unwrap())
                .unwrap();
            assert!(big.1 > &0.0, "PC{} largest loading is negative", j + 1);
            for i in 0..10 {
                assert!(
                    (a.loadings[j][i] - b.loadings[j][i]).abs() < 1e-9,
                    "PC{} cell {i} disagrees after a sign flip",
                    j + 1
                );
            }
        }
    }

    #[test]
    fn full_rank_reconstruction_is_exact() {
        let p = synthetic(200, 8, 0.4);
        let fit = fit(&p, 8).expect("fit");
        // With every component retained, scores must rebuild each row exactly
        // -- which is what proves the standardisation inverts correctly.
        for (i, row) in p.iter().enumerate() {
            let got = fit.reconstruct(&fit.scores[i]);
            for (c, (g, w)) in got.iter().zip(row.iter()).enumerate() {
                assert!((g - w).abs() < 1e-9, "row {i} cell {c}: {g} vs {w}");
            }
        }
    }

    #[test]
    fn truncated_reconstruction_beats_the_mean() {
        let p = synthetic(300, 12, 0.15);
        let full = fit(&p, 12).expect("fit");
        let three = fit(&p, 3).expect("fit");
        let err = |f: &PcaFit| -> f64 {
            p.iter()
                .enumerate()
                .map(|(i, row)| {
                    f.reconstruct(&f.scores[i])
                        .iter()
                        .zip(row)
                        .map(|(g, w)| (g - w) * (g - w))
                        .sum::<f64>()
                })
                .sum::<f64>()
        };
        let baseline: f64 = p
            .iter()
            .map(|row| row.iter().zip(&full.mean).map(|(w, m)| (w - m) * (w - m)).sum::<f64>())
            .sum();
        assert!(err(&full) < 1e-12 * baseline.max(1.0), "full rank should be exact");
        assert!(err(&three) < 0.25 * baseline, "3 factors should beat the mean substantially");
    }

    #[test]
    fn shock_keeps_volatility_positive() {
        let fit = fit(&synthetic(300, 6, 0.2), 3).expect("fit");
        let base = vec![0.45, 0.42, 0.40, 0.38, 0.37, 0.36];
        // A brutal ten-sigma move on every factor: additive changes would go
        // straight through zero here, log changes cannot.
        let shocked = fit.shock(&base, &[10.0, -10.0, 10.0]);
        assert!(shocked.iter().all(|v| *v > 0.0 && v.is_finite()), "{shocked:?}");
        let calm = fit.shock(&base, &[0.0, 0.0, 0.0]);
        for (c, b) in calm.iter().zip(&base) {
            // Zero scores still carry the window's mean drift, which is tiny.
            assert!((c - b).abs() / b < 0.05, "{c} vs {b}");
        }
    }

    #[test]
    fn rejects_unusable_panels() {
        assert!(matches!(
            fit(&synthetic(10, 5, 0.1), 2),
            Err(PcaError::TooFewObservations { .. })
        ));
        assert!(matches!(fit(&[], 2), Err(PcaError::TooFewObservations { .. })));
        let constant: Vec<Vec<f64>> = (0..60).map(|_| vec![1.0, 2.0, 3.0]).collect();
        assert!(matches!(fit(&constant, 2), Err(PcaError::ConstantColumn(_))));
        let mut ragged = synthetic(60, 5, 0.1);
        ragged[3].pop();
        assert!(matches!(fit(&ragged, 2), Err(PcaError::Malformed)));
        assert!(matches!(
            fit(&synthetic(60, 5, 0.1), 9),
            Err(PcaError::TooManyComponents { .. })
        ));
    }
}
