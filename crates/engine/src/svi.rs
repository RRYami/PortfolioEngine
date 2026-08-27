//! SVI: a five-parameter smile per expiry slice.
//!
//! Fits Gatheral's raw parameterisation of *total implied variance* against
//! log-forward-moneyness:
//!
//! ```text
//! w(k) = a + b*( rho*(k-m) + sqrt((k-m)^2 + sigma^2) )
//! ```
//!
//! Total variance rather than volatility because the no-arbitrage conditions
//! are natural in it: calendar arbitrage is just `w` non-decreasing in
//! maturity at fixed `k`, and Durrleman's butterfly condition is a statement
//! about `w` and its first two derivatives.
//!
//! Five parameters replace the 40-110 raw quotes in a slice, and — unlike the
//! point cloud — the result can be evaluated *between* listed strikes. That is
//! not cosmetic: a constant-maturity grid has to be read at fixed moneyness
//! points that almost never coincide with a listed strike, so everything
//! downstream of here depends on the slice being a function rather than a set
//! of dots.

use std::fmt;

/// Raw SVI parameters for one expiry.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Svi {
    /// Overall level of variance.
    pub a: f64,
    /// Angle between the wings; `b >= 0`.
    pub b: f64,
    /// Skew, `|rho| < 1`; negative tilts variance toward low strikes.
    pub rho: f64,
    /// Horizontal shift of the smile's minimum.
    pub m: f64,
    /// Curvature at the minimum; `sigma > 0`.
    pub sigma: f64,
}

impl Svi {
    /// Total implied variance at log-moneyness `k`.
    #[must_use]
    pub fn total_variance(&self, k: f64) -> f64 {
        let d = k - self.m;
        self.a + self.b * (self.rho * d + (d * d + self.sigma * self.sigma).sqrt())
    }

    /// Implied volatility at `k` for a slice of maturity `tte`.
    #[must_use]
    pub fn vol(&self, k: f64, tte: f64) -> f64 {
        if tte <= 0.0 {
            return 0.0;
        }
        (self.total_variance(k).max(0.0) / tte).sqrt()
    }

    /// First and second derivative of `w` in `k`.
    #[must_use]
    pub fn derivatives(&self, k: f64) -> (f64, f64) {
        let d = k - self.m;
        let r = (d * d + self.sigma * self.sigma).sqrt();
        let w1 = self.b * (self.rho + d / r);
        let w2 = self.b * self.sigma * self.sigma / (r * r * r);
        (w1, w2)
    }

    /// Durrleman's function. Negative anywhere means the slice admits a
    /// butterfly arbitrage — a call spread with negative implied density.
    #[must_use]
    pub fn durrleman(&self, k: f64) -> f64 {
        let w = self.total_variance(k);
        if w <= 0.0 {
            return f64::NEG_INFINITY;
        }
        let (w1, w2) = self.derivatives(k);
        let t = 1.0 - k * w1 / (2.0 * w);
        t * t - 0.25 * w1 * w1 * (1.0 / w + 0.25) + 0.5 * w2
    }

    /// Scan `[lo, hi]` for a butterfly violation.
    ///
    /// Sampled rather than solved: `g` has no closed-form root for SVI, and a
    /// dense scan over the range a surface is actually read at is both simpler
    /// and sufficient — the fit only has to be arbitrage-free where it is used.
    #[must_use]
    pub fn butterfly_free(&self, lo: f64, hi: f64) -> bool {
        self.min_durrleman(lo, hi) >= 0.0
    }

    /// Worst value of Durrleman's function over `[lo, hi]`.
    #[must_use]
    pub fn min_durrleman(&self, lo: f64, hi: f64) -> f64 {
        const STEPS: u32 = 200;
        let mut worst = f64::INFINITY;
        for i in 0..=STEPS {
            let k = (hi - lo).mul_add(f64::from(i) / f64::from(STEPS), lo);
            worst = worst.min(self.durrleman(k));
        }
        worst
    }

    /// The parameter-space conditions that are checkable without a scan.
    #[must_use]
    pub fn params_sane(&self) -> bool {
        self.b >= 0.0
            && self.rho.abs() < 1.0
            && self.sigma > 0.0
            && self.a + self.b * self.sigma * (1.0 - self.rho * self.rho).sqrt() >= 0.0
    }
}

/// A calibrated slice with the diagnostics needed to decide whether to use it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SviFit {
    pub params: Svi,
    /// Weighted RMS residual in *total variance* units.
    pub rmse: f64,
    /// Same residual expressed in volatility points at this maturity, which is
    /// the number anyone reading a surface actually thinks in.
    pub rmse_vol: f64,
    pub points: usize,
    /// Worst Durrleman value across the fitted range; negative means the smile
    /// admits a butterfly arbitrage somewhere it is used.
    pub min_durrleman: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SviError {
    TooFewPoints { got: usize, need: usize },
    /// Every point had zero or non-finite weight, or all `k` were identical.
    Degenerate,
    /// The outer search never produced a usable inner solution.
    NoFit,
}

impl fmt::Display for SviError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewPoints { got, need } => write!(f, "{got} points, need {need}"),
            Self::Degenerate => write!(f, "points are degenerate in k or weight"),
            Self::NoFit => write!(f, "no usable fit found"),
        }
    }
}

impl std::error::Error for SviError {}

/// Five parameters need at least this many points to be worth fitting.
pub const MIN_POINTS: usize = 6;

/// Weight on the butterfly penalty in the calibration objective.
const ARB_PENALTY: f64 = 1e-3;

/// Wing-steepness bounds tried in order, as multiples of `sigma` in the
/// reduced problem (`c + |d| <= wing * sigma`).
///
/// The first is Lee's no-arbitrage bound, `b(1 + |rho|) <= 4`. That bounds the
/// wings but is not sufficient for the butterfly condition at short tenors:
/// total variance there is ~0.01, so the `1/w` term in Durrleman's function is
/// ~100 and even a modest skew can drive it negative. Rather than accept an
/// arbitrageable smile or flatten every slice, the calibration retries with
/// progressively tighter wings and keeps the *loosest* one that comes out
/// arbitrage-free — the least-flattened admissible fit.
const WING_BOUNDS: [f64; 5] = [4.0, 2.0, 1.0, 0.5, 0.25];

/// One observation: log-moneyness, total variance, and a confidence weight.
#[derive(Debug, Clone, Copy)]
pub struct SlicePoint {
    pub k: f64,
    pub w: f64,
    pub weight: f64,
}

/// Calibrate a slice.
///
/// Uses the quasi-explicit split: with `m` and `sigma` held fixed, substituting
/// `y = (k-m)/sigma` makes the model **linear** in `(a, d, c)` where `c = b*sigma`
/// and `d = rho*b*sigma`:
///
/// ```text
/// w = a + d*y + c*sqrt(y^2 + 1)
/// ```
///
/// So the inner problem is an exactly-solved constrained least squares and only
/// the outer two dimensions need searching. Fitting all five at once with a
/// general optimiser is what makes SVI notorious for landing in local minima;
/// this reduces the search to a plane.
pub fn calibrate(points: &[SlicePoint], tte: f64) -> Result<SviFit, SviError> {
    if points.len() < MIN_POINTS {
        return Err(SviError::TooFewPoints { got: points.len(), need: MIN_POINTS });
    }
    let usable: Vec<SlicePoint> = points
        .iter()
        .copied()
        .filter(|p| p.weight > 0.0 && p.k.is_finite() && p.w.is_finite() && p.w > 0.0)
        .collect();
    if usable.len() < MIN_POINTS {
        return Err(SviError::TooFewPoints { got: usable.len(), need: MIN_POINTS });
    }
    let (k_lo, k_hi) = usable.iter().fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.k), h.max(p.k)));
    if k_hi <= k_lo || !(k_lo.is_finite() && k_hi.is_finite()) {
        return Err(SviError::Degenerate);
    }
    let w_max = usable.iter().fold(0.0_f64, |m, p| m.max(p.w));
    let span = k_hi - k_lo;

    let sw: f64 = usable.iter().map(|p| p.weight).sum();

    // Outer search over (m, log sigma). Nelder-Mead from several starts,
    // because the reduced surface is smooth but not convex.
    //
    // The objective carries a butterfly penalty rather than only reporting
    // violations afterwards. Left unpenalised this fit produced a negative
    // Durrleman value on 5.5% of sub-two-month slices -- short tenors have few
    // points and tiny total variance, which makes `g` extremely sensitive --
    // and a smile with negative implied density is not something to hand
    // downstream and hope nobody integrates against it. The penalty is scaled
    // by the weight sum so it stays commensurate with the residual whatever
    // the slice's units of confidence.
    let search = |wing: f64| -> Option<(Svi, f64)> {
    let objective = |v: [f64; 2]| -> f64 {
        let (m, sigma) = (v[0], v[1].exp());
        let Some((x, sse)) = inner(&usable, m, sigma, w_max, wing) else { return f64::INFINITY };
        let c = x[2];
        let cand = Svi {
            a: x[0],
            b: c / sigma,
            rho: if c > 0.0 { (x[1] / c).clamp(-0.999_999, 0.999_999) } else { 0.0 },
            m,
            sigma,
        };
        let violation = (-cand.min_durrleman(k_lo, k_hi)).max(0.0);
        sse + ARB_PENALTY * sw * violation * violation
    };
        let mut best: Option<([f64; 2], f64)> = None;
        for &m0 in &[k_lo, 0.5 * (k_lo + k_hi), k_hi, 0.0] {
            for &s0 in &[0.1 * span, 0.4 * span, span] {
                if s0 <= 0.0 {
                    continue;
                }
                let (v, f) = nelder_mead(&objective, [m0, s0.ln()], [0.1 * span, 0.5]);
                if f.is_finite() && best.is_none_or(|(_, bf)| f < bf) {
                    best = Some((v, f));
                }
            }
        }
        let (v, _) = best?;
        let (m, sigma) = (v[0], v[1].exp());
        // Recover the *unpenalised* residual: the objective above includes the
        // arbitrage term, and reporting that as fit error would misstate how
        // well the smile tracks the quotes.
        let (x, sse) = inner(&usable, m, sigma, w_max, wing)?;
        let c = x[2];
        Some((
            Svi {
                a: x[0],
                b: c / sigma,
                rho: if c > 0.0 { (x[1] / c).clamp(-0.999_999, 0.999_999) } else { 0.0 },
                m,
                sigma,
            },
            sse,
        ))
    };

    // Loosest arbitrage-free fit wins; if none is, keep the least-violating.
    let mut fallback: Option<(Svi, f64, f64)> = None;
    let mut chosen: Option<(Svi, f64)> = None;
    for &wing in &WING_BOUNDS {
        let Some((cand, sse)) = search(wing) else { continue };
        let g = cand.min_durrleman(k_lo, k_hi);
        if g >= 0.0 {
            chosen = Some((cand, sse));
            break;
        }
        if fallback.as_ref().is_none_or(|&(_, _, bg)| g > bg) {
            fallback = Some((cand, sse, g));
        }
    }
    let (params, sse) = chosen
        .or_else(|| fallback.map(|(p, s, _)| (p, s)))
        .ok_or(SviError::NoFit)?;

    let rmse = (sse / sw).sqrt();
    // Convert a variance residual to vol points at the slice's own level:
    // dw = 2*sigma_bs*T*d(sigma_bs), so d(sigma) = dw / (2*sigma_bs*T).
    let atm_vol = params.vol(0.0, tte).max(1e-6);
    let rmse_vol = if tte > 0.0 { rmse / (2.0 * atm_vol * tte) } else { 0.0 };

    Ok(SviFit {
        params,
        rmse,
        rmse_vol,
        points: usable.len(),
        min_durrleman: params.min_durrleman(k_lo, k_hi),
    })
}

/// Constrained linear least squares in `(a, d, c)` for fixed `(m, sigma)`.
///
/// The box comes from the no-arbitrage conditions carried into the reduced
/// variables: `b >= 0` gives `c >= 0`, `|rho| <= 1` gives `|d| <= c`, and
/// Lee's wing bound `b(1+|rho|) <= 4` becomes `c + |d| <= 4*sigma`.
fn inner(
    pts: &[SlicePoint],
    m: f64,
    sigma: f64,
    w_max: f64,
    wing: f64,
) -> Option<([f64; 3], f64)> {
    if sigma <= 0.0 || !sigma.is_finite() || !m.is_finite() {
        return None;
    }
    // Normal equations for the three basis functions 1, y, sqrt(y^2+1).
    let mut ata = [[0.0_f64; 3]; 3];
    let mut atb = [0.0_f64; 3];
    for p in pts {
        let y = (p.k - m) / sigma;
        let phi = [1.0, y, (y * y + 1.0).sqrt()];
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += p.weight * phi[i] * phi[j];
            }
            atb[i] += p.weight * phi[i] * p.w;
        }
    }
    let mut x = solve3(ata, atb)?;

    // Project onto the feasible set, then re-solve the still-free coordinates
    // once with the clamped ones fixed. Cheap, and enough in practice because
    // the objective is convex and the box is tight around the optimum.
    let clamp = |x: &mut [f64; 3]| -> bool {
        let mut hit = false;
        if x[2] < 0.0 {
            x[2] = 0.0;
            hit = true;
        }
        if x[2] > wing * sigma {
            x[2] = wing * sigma;
            hit = true;
        }
        let lim = x[2].min(wing * sigma - x[2]).max(0.0);
        if x[1].abs() > lim {
            x[1] = x[1].clamp(-lim, lim);
            hit = true;
        }
        if x[0] < 0.0 {
            x[0] = 0.0;
            hit = true;
        }
        if x[0] > w_max {
            x[0] = w_max;
            hit = true;
        }
        hit
    };
    if clamp(&mut x) {
        // Re-fit `a` alone against the clamped wings — the level is the
        // parameter the residual is most sensitive to.
        let (mut num, mut den) = (0.0, 0.0);
        for p in pts {
            let y = (p.k - m) / sigma;
            let rest = x[1] * y + x[2] * (y * y + 1.0).sqrt();
            num += p.weight * (p.w - rest);
            den += p.weight;
        }
        if den > 0.0 {
            x[0] = (num / den).clamp(0.0, w_max);
        }
    }

    let mut sse = 0.0;
    for p in pts {
        let y = (p.k - m) / sigma;
        let fit = x[0] + x[1] * y + x[2] * (y * y + 1.0).sqrt();
        sse += p.weight * (fit - p.w) * (fit - p.w);
    }
    sse.is_finite().then_some((x, sse))
}

/// Gaussian elimination with partial pivoting on a 3x3 system.
fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..3 {
        let piv = (col..3).max_by(|&i, &j| {
            a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap_or(std::cmp::Ordering::Equal)
        })?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in (col + 1)..3 {
            let f = a[row][col] / a[col][col];
            let pivot_row = a[col];
            for (dst, src) in a[row].iter_mut().zip(pivot_row.iter()).skip(col) {
                *dst -= f * src;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 3];
    for i in (0..3).rev() {
        let mut sum = b[i];
        for (coef, xv) in a[i].iter().zip(x.iter()).skip(i + 1) {
            sum -= coef * xv;
        }
        x[i] = sum / a[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Compact Nelder-Mead in two dimensions.
fn nelder_mead<F: Fn([f64; 2]) -> f64>(f: &F, start: [f64; 2], step: [f64; 2]) -> ([f64; 2], f64) {
    const MAX_ITER: usize = 200;
    let mut s = [
        start,
        [start[0] + step[0], start[1]],
        [start[0], start[1] + step[1]],
    ];
    let mut v = [f(s[0]), f(s[1]), f(s[2])];
    for _ in 0..MAX_ITER {
        let mut idx = [0, 1, 2];
        idx.sort_by(|&i, &j| v[i].partial_cmp(&v[j]).unwrap_or(std::cmp::Ordering::Equal));
        let (best, mid, worst) = (idx[0], idx[1], idx[2]);
        if (v[worst] - v[best]).abs() <= 1e-12 * (1.0 + v[best].abs()) {
            break;
        }
        let cen = [
            0.5 * (s[best][0] + s[mid][0]),
            0.5 * (s[best][1] + s[mid][1]),
        ];
        let refl = [
            2.0f64.mul_add(cen[0], -s[worst][0]),
            2.0f64.mul_add(cen[1], -s[worst][1]),
        ];
        let fr = f(refl);
        if fr < v[best] {
            let exp = [3.0 * cen[0] - 2.0 * s[worst][0], 3.0 * cen[1] - 2.0 * s[worst][1]];
            let fe = f(exp);
            if fe < fr {
                s[worst] = exp;
                v[worst] = fe;
            } else {
                s[worst] = refl;
                v[worst] = fr;
            }
        } else if fr < v[mid] {
            s[worst] = refl;
            v[worst] = fr;
        } else {
            let con = [
                0.5 * (cen[0] + s[worst][0]),
                0.5 * (cen[1] + s[worst][1]),
            ];
            let fc = f(con);
            if fc < v[worst] {
                s[worst] = con;
                v[worst] = fc;
            } else {
                for i in [mid, worst] {
                    s[i] = [0.5 * (s[i][0] + s[best][0]), 0.5 * (s[i][1] + s[best][1])];
                    v[i] = f(s[i]);
                }
            }
        }
    }
    let b = (0..3).min_by(|&i, &j| v[i].partial_cmp(&v[j]).unwrap_or(std::cmp::Ordering::Equal));
    b.map_or((start, f64::INFINITY), |b| (s[b], v[b]))
}

#[cfg(test)]
#[allow(clippy::unreadable_literal)] // REAL_SLICE is a generated fixture
mod tests {
    use super::*;

    /// SOXX 2026-08-20, expiry 2026-09-25: (log-moneyness, implied vol).
    const REAL_SLICE: &[(f64, f64)] = &[
    (-0.521829, 0.752591),
    (-0.459309, 0.695182),
    (-0.414857, 0.684463),
    (-0.400469, 0.654598),
    (-0.372298, 0.625934),
    (-0.358504, 0.597526),
    (-0.344899, 0.598800),
    (-0.331476, 0.592664),
    (-0.318231, 0.594939),
    (-0.305158, 0.574422),
    (-0.292255, 0.544754),
    (-0.279516, 0.568186),
    (-0.266937, 0.550188),
    (-0.254515, 0.546999),
    (-0.242245, 0.538243),
    (-0.230123, 0.528393),
    (-0.218147, 0.527940),
    (-0.206313, 0.511771),
    (-0.194617, 0.504400),
    (-0.183056, 0.507247),
    (-0.171627, 0.495057),
    (-0.160328, 0.484917),
    (-0.149154, 0.480451),
    (-0.138104, 0.482949),
    (-0.127175, 0.480409),
    (-0.116364, 0.468070),
    (-0.105669, 0.447149),
    (-0.095087, 0.457318),
    (-0.084616, 0.455481),
    (-0.074253, 0.447839),
    (-0.063996, 0.437605),
    (-0.053844, 0.443669),
    (-0.048806, 0.435221),
    (-0.043794, 0.431396),
    (-0.038806, 0.430358),
    (-0.033843, 0.437627),
    (-0.028905, 0.429037),
    (-0.023991, 0.429518),
    (-0.019101, 0.430994),
    (-0.014235, 0.429559),
    (-0.009392, 0.430680),
    (-0.004573, 0.418159),
    (0.000223, 0.423838),
    (0.004996, 0.417125),
    (0.009747, 0.418298),
    (0.014475, 0.415095),
    (0.019181, 0.412103),
    (0.023865, 0.413176),
    (0.028527, 0.413720),
    (0.033167, 0.413736),
    (0.037786, 0.414008),
    (0.042384, 0.409796),
    (0.046961, 0.408981),
    (0.051516, 0.408436),
    (0.056052, 0.408179),
    (0.060566, 0.413219),
    (0.065061, 0.412836),
    (0.069535, 0.405932),
    (0.073989, 0.411384),
    (0.078424, 0.410312),
    (0.082839, 0.403248),
    (0.087235, 0.404678),
    (0.091611, 0.407571),
    (0.095968, 0.411067),
    (0.104626, 0.406968),
    (0.113210, 0.413309),
    (0.121721, 0.408315),
    (0.130160, 0.411599),
    (0.138528, 0.404469),
    (0.146827, 0.401308),
    (0.155057, 0.416546),
    (0.163220, 0.403056),
    (0.171318, 0.407048),
    (0.179350, 0.405235),
    (0.187318, 0.403387),
    (0.195223, 0.411788),
    (0.203066, 0.416287),
    (0.210849, 0.419927),
    (0.218571, 0.412974),
    (0.226233, 0.422814),
    (0.233838, 0.421496),
    (0.241385, 0.400472),
    (0.248876, 0.428723),
    (0.292679, 0.441741),
    ];
    const REAL_TTE: f64 = 0.098_563;

    fn pts(ks: &[f64], svi: &Svi) -> Vec<SlicePoint> {
        ks.iter().map(|&k| SlicePoint { k, w: svi.total_variance(k), weight: 1.0 }).collect()
    }

    fn grid(lo: f64, hi: f64, n: u32) -> Vec<f64> {
        (0..n).map(|i| (hi - lo).mul_add(f64::from(i) / f64::from(n - 1), lo)).collect()
    }

    #[test]
    fn recovers_a_known_smile() {
        // A realistic equity slice: downside skew, mild curvature.
        let truth = Svi { a: 0.012, b: 0.085, rho: -0.62, m: 0.018, sigma: 0.13 };
        let ks = grid(-0.45, 0.30, 30);
        let fit = calibrate(&pts(&ks, &truth), 0.25).expect("fit");
        // SVI is weakly identifiable from a finite sample, so compare the
        // curve, not the parameters.
        for &k in &ks {
            let (got, want) = (fit.params.total_variance(k), truth.total_variance(k));
            assert!((got - want).abs() < 1e-6, "k={k}: {got} vs {want}");
        }
        assert!(fit.rmse < 1e-6, "rmse {}", fit.rmse);
    }

    #[test]
    fn fits_a_real_slice_within_the_noise_floor() {
        let points: Vec<SlicePoint> = REAL_SLICE
            .iter()
            .map(|&(k, iv)| SlicePoint { k, w: iv * iv * REAL_TTE, weight: 1.0 })
            .collect();
        let fit = calibrate(&points, REAL_TTE).expect("fit");
        assert_eq!(fit.points, REAL_SLICE.len());
        // The raw cloud's own point-to-point chop measured ~0.003 vol at p90,
        // so a fit that lands inside a vol point is tracking the smile rather
        // than the noise.
        assert!(fit.rmse_vol < 0.01, "rmse {} vol points", fit.rmse_vol);
        assert!(fit.params.params_sane(), "params {:?}", fit.params);
        assert!(fit.min_durrleman >= 0.0, "butterfly arb: g_min {}", fit.min_durrleman);
        // Equity skew: variance must fall as strike rises, near the money.
        assert!(
            fit.params.total_variance(-0.1) > fit.params.total_variance(0.1),
            "expected downside skew"
        );
    }

    #[test]
    fn durrleman_flags_a_butterfly_arbitrage() {
        // Wings far too steep for the level: b(1+|rho|) well past Lee's bound.
        let bad = Svi { a: 0.001, b: 3.0, rho: -0.9, m: 0.0, sigma: 0.02 };
        assert!(!bad.butterfly_free(-0.5, 0.5));
        let good = Svi { a: 0.012, b: 0.085, rho: -0.62, m: 0.018, sigma: 0.13 };
        assert!(good.butterfly_free(-0.5, 0.5), "g_min {}", good.min_durrleman(-0.5, 0.5));
    }

    #[test]
    fn vol_and_variance_agree() {
        let s = Svi { a: 0.012, b: 0.085, rho: -0.62, m: 0.018, sigma: 0.13 };
        let t = 0.25;
        for k in [-0.3, 0.0, 0.2] {
            let v = s.vol(k, t);
            assert!((v * v * t - s.total_variance(k)).abs() < 1e-12);
        }
        assert!((s.vol(0.0, 0.0) - 0.0).abs() < 1e-12, "zero tenor is a limit, not an error");
    }

    #[test]
    fn rejects_unusable_input() {
        let s = Svi { a: 0.01, b: 0.08, rho: -0.5, m: 0.0, sigma: 0.1 };
        assert!(matches!(
            calibrate(&pts(&grid(-0.2, 0.2, 3), &s), 0.25),
            Err(SviError::TooFewPoints { .. })
        ));
        let flat: Vec<SlicePoint> =
            (0..10).map(|_| SlicePoint { k: 0.0, w: 0.01, weight: 1.0 }).collect();
        assert!(matches!(calibrate(&flat, 0.25), Err(SviError::Degenerate)));
        let zero_weight: Vec<SlicePoint> =
            grid(-0.2, 0.2, 10).iter().map(|&k| SlicePoint { k, w: 0.01, weight: 0.0 }).collect();
        assert!(matches!(calibrate(&zero_weight, 0.25), Err(SviError::TooFewPoints { .. })));
    }

    #[test]
    fn weights_pull_the_fit_toward_trusted_points() {
        let truth = Svi { a: 0.012, b: 0.085, rho: -0.62, m: 0.018, sigma: 0.13 };
        let ks = grid(-0.4, 0.3, 24);
        let mut points = pts(&ks, &truth);
        // Corrupt the far wing badly, but tell the fit not to trust it.
        for p in &mut points {
            if p.k < -0.3 {
                p.w *= 2.0;
                p.weight = 1e-4;
            }
        }
        let fit = calibrate(&points, 0.25).expect("fit");
        let err = fit.params.total_variance(0.0) - truth.total_variance(0.0);
        assert!(err.abs() < 1e-3, "at-the-money dragged by {err}");
    }
}
