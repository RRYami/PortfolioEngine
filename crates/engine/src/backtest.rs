//! Backtesting a `VaR` model against what actually happened.
//!
//! A `VaR` number is a claim with a testable frequency: at 99% confidence the
//! loss should exceed it on about one day in a hundred. Two things can go
//! wrong, and they need separate tests because a model can pass one while
//! failing the other.
//!
//! *Coverage* asks whether there were the right **number** of exceptions.
//! Kupiec's proportion-of-failures test answers it.
//!
//! *Independence* asks whether they arrived at the right **times**. A model
//! that is fine on quiet days and breaks for a week during a selloff can show
//! textbook coverage while being useless, because the exceptions cluster.
//! Christoffersen's test answers that, and the two combine into a conditional
//! coverage statistic.

use std::fmt;

/// One day of the backtest: what the model promised, and what happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// The loss the model said would not be exceeded, positive.
    pub var: f64,
    /// The loss that occurred, positive for a loss.
    pub loss: f64,
}

impl Outcome {
    #[must_use]
    pub fn is_exception(&self) -> bool {
        self.loss > self.var
    }
}

/// Result of a coverage and independence assessment.
#[derive(Debug, Clone, PartialEq)]
pub struct BacktestReport {
    pub confidence: f64,
    pub observations: usize,
    pub exceptions: usize,
    /// Realised exception rate.
    pub rate: f64,
    /// Rate the model claims, `1 - confidence`.
    pub expected_rate: f64,
    /// Kupiec proportion-of-failures statistic, chi-square with 1 d.f.
    pub kupiec: f64,
    /// Christoffersen independence statistic, chi-square with 1 d.f.
    pub independence: f64,
    /// Conditional coverage, the sum of the two, chi-square with 2 d.f.
    pub conditional_coverage: f64,
    /// Transition counts between quiet and exception days: `n[i][j]` is a move
    /// from state `i` to state `j`, where 1 means an exception.
    pub transitions: [[usize; 2]; 2],
}

/// Critical values at 95% for chi-square with one and two degrees of freedom.
///
/// Hard-coded rather than computed: two constants from a table are clearer
/// than an incomplete-gamma implementation nothing else in the crate needs.
pub const CHI2_95_1DF: f64 = 3.841;
pub const CHI2_95_2DF: f64 = 5.991;

impl BacktestReport {
    /// Whether the exception count is consistent with the claimed confidence.
    #[must_use]
    pub fn coverage_ok(&self) -> bool {
        self.kupiec <= CHI2_95_1DF
    }

    /// Whether exceptions arrive independently rather than in clusters.
    #[must_use]
    pub fn independence_ok(&self) -> bool {
        self.independence <= CHI2_95_1DF
    }

    /// Both together — the test a model has to pass to be usable.
    #[must_use]
    pub fn passes(&self) -> bool {
        self.conditional_coverage <= CHI2_95_2DF
    }
}

impl fmt::Display for BacktestReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:.0}%: {}/{} exceptions ({:.2}% vs {:.2}% expected); \
             Kupiec {:.2} {}, independence {:.2} {}, conditional {:.2} {}",
            self.confidence * 100.0,
            self.exceptions,
            self.observations,
            self.rate * 100.0,
            self.expected_rate * 100.0,
            self.kupiec,
            if self.coverage_ok() { "pass" } else { "FAIL" },
            self.independence,
            if self.independence_ok() { "pass" } else { "FAIL" },
            self.conditional_coverage,
            if self.passes() { "pass" } else { "FAIL" },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BacktestError {
    /// Too few days for the statistics to mean anything.
    TooFewObservations { got: usize, need: usize },
    /// Confidence outside `(0, 1)`.
    BadConfidence(f64),
}

impl fmt::Display for BacktestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewObservations { got, need } => {
                write!(f, "{got} observations, need at least {need}")
            }
            Self::BadConfidence(c) => write!(f, "confidence {c} is not in (0, 1)"),
        }
    }
}

impl std::error::Error for BacktestError {}

/// Fewest days worth testing. Below this the chi-square approximation the two
/// tests rest on is not trustworthy, and a single exception swings the verdict.
pub const MIN_OBSERVATIONS: usize = 100;

/// Assess a sequence of daily outcomes.
pub fn assess(outcomes: &[Outcome], confidence: f64) -> Result<BacktestReport, BacktestError> {
    if !(confidence > 0.0 && confidence < 1.0) {
        return Err(BacktestError::BadConfidence(confidence));
    }
    let n = outcomes.len();
    if n < MIN_OBSERVATIONS {
        return Err(BacktestError::TooFewObservations { got: n, need: MIN_OBSERVATIONS });
    }
    let hits: Vec<bool> = outcomes.iter().map(Outcome::is_exception).collect();
    let x = hits.iter().filter(|h| **h).count();
    #[allow(clippy::cast_precision_loss)]
    let (nf, xf) = (n as f64, x as f64);
    let p = 1.0 - confidence;
    let rate = xf / nf;

    // Kupiec: twice the log-likelihood ratio of the observed failure rate
    // against the claimed one.
    let kupiec = if x == 0 {
        // The limit as the observed rate goes to zero; the general form has a
        // 0*ln(0) in it.
        -2.0 * nf * (1.0 - p).ln()
    } else if x == n {
        -2.0 * nf * p.ln()
    } else {
        let ln_null = (nf - xf) * (1.0 - p).ln() + xf * p.ln();
        let ln_alt = (nf - xf) * (1.0 - rate).ln() + xf * rate.ln();
        -2.0 * (ln_null - ln_alt)
    };

    // Christoffersen: does an exception today change the odds of one tomorrow?
    let mut t = [[0usize; 2]; 2];
    for w in hits.windows(2) {
        t[usize::from(w[0])][usize::from(w[1])] += 1;
    }
    let independence = independence_statistic(t);

    Ok(BacktestReport {
        confidence,
        observations: n,
        exceptions: x,
        rate,
        expected_rate: p,
        kupiec: kupiec.max(0.0),
        independence,
        conditional_coverage: kupiec.max(0.0) + independence,
        transitions: t,
    })
}

/// Christoffersen's Markov independence statistic.
///
/// Compares a first-order chain, where the odds of an exception depend on
/// whether yesterday was one, against a memoryless model. Returns zero when a
/// transition is unobserved: with no evidence of dependence there is nothing to
/// reject, and the alternative's likelihood is degenerate.
#[allow(clippy::cast_precision_loss)]
fn independence_statistic(t: [[usize; 2]; 2]) -> f64 {
    let (n00, n01, n10, n11) = (t[0][0] as f64, t[0][1] as f64, t[1][0] as f64, t[1][1] as f64);
    let total = n00 + n01 + n10 + n11;
    if total <= 0.0 || n01 + n11 <= 0.0 {
        return 0.0;
    }
    let pi = (n01 + n11) / total;
    let pi0 = if n00 + n01 > 0.0 { n01 / (n00 + n01) } else { 0.0 };
    let pi1 = if n10 + n11 > 0.0 { n11 / (n10 + n11) } else { 0.0 };
    // A degenerate branch means the data cannot distinguish the two models.
    if pi <= 0.0 || pi >= 1.0 || pi0 <= 0.0 || pi1 <= 0.0 || pi0 >= 1.0 || pi1 >= 1.0 {
        return 0.0;
    }
    let ln_null = (n00 + n10) * (1.0 - pi).ln() + (n01 + n11) * pi.ln();
    let ln_alt = n00 * (1.0 - pi0).ln()
        + n01 * pi0.ln()
        + n10 * (1.0 - pi1).ln()
        + n11 * pi1.ln();
    (-2.0 * (ln_null - ln_alt)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic exception pattern: every `period`-th day breaches.
    fn spaced(n: usize, period: usize) -> Vec<Outcome> {
        (0..n)
            .map(|i| Outcome {
                var: 100.0,
                loss: if period > 0 && i % period == 0 { 150.0 } else { 50.0 },
            })
            .collect()
    }

    #[test]
    fn a_correctly_calibrated_model_passes() {
        // 99% confidence with an exception every hundredth day.
        let r = assess(&spaced(500, 100), 0.99).expect("assess");
        assert_eq!(r.exceptions, 5);
        assert!((r.rate - 0.01).abs() < 1e-12);
        assert!(r.coverage_ok(), "kupiec {}", r.kupiec);
        assert!(r.independence_ok(), "independence {}", r.independence);
        assert!(r.passes(), "{r}");
    }

    #[test]
    fn too_many_exceptions_fail_coverage() {
        // One day in ten against a 99% claim: the model is badly optimistic.
        let r = assess(&spaced(500, 10), 0.99).expect("assess");
        assert_eq!(r.exceptions, 50);
        assert!(!r.coverage_ok(), "kupiec {} should reject", r.kupiec);
        assert!(!r.passes());
    }

    #[test]
    fn too_few_exceptions_also_fail_coverage() {
        // A model that never breaches is not conservative, it is wrong: it
        // overstates risk and would tie up capital for nothing.
        let none: Vec<Outcome> = (0..500).map(|_| Outcome { var: 1e9, loss: 1.0 }).collect();
        let r = assess(&none, 0.95).expect("assess");
        assert_eq!(r.exceptions, 0);
        assert!(!r.coverage_ok(), "kupiec {} should reject zero exceptions", r.kupiec);
    }

    /// The case coverage alone cannot see.
    #[test]
    fn clustered_exceptions_fail_independence_despite_right_count() {
        let mut o: Vec<Outcome> = (0..500).map(|_| Outcome { var: 100.0, loss: 50.0 }).collect();
        // Exactly 25 exceptions — dead on 5% — but all consecutive.
        for item in o.iter_mut().skip(100).take(25) {
            item.loss = 150.0;
        }
        let r = assess(&o, 0.95).expect("assess");
        assert_eq!(r.exceptions, 25);
        assert!(r.coverage_ok(), "count is exactly right: kupiec {}", r.kupiec);
        assert!(
            !r.independence_ok(),
            "24 of 25 exceptions follow an exception; independence {} should reject",
            r.independence
        );
        assert!(!r.passes(), "conditional coverage must catch what coverage alone misses");
    }

    #[test]
    fn transitions_are_counted_correctly() {
        // Pattern: quiet, hit, hit, quiet, quiet -> 0->1, 1->1, 1->0, 0->0
        let losses = [50.0, 150.0, 150.0, 50.0, 50.0];
        let o: Vec<Outcome> = losses
            .iter()
            .cycle()
            .take(300)
            .map(|&loss| Outcome { var: 100.0, loss })
            .collect();
        let r = assess(&o, 0.95).expect("assess");
        let t = r.transitions;
        assert_eq!(t[0][1] + t[1][1], r.exceptions - usize::from(o[0].is_exception()));
        assert!(t[1][1] > 0, "the pattern has back-to-back exceptions");
        assert_eq!(t.iter().flatten().sum::<usize>(), o.len() - 1);
    }

    #[test]
    fn independence_is_silent_without_evidence() {
        // No exceptions at all: nothing to say about their arrival pattern.
        let none: Vec<Outcome> = (0..300).map(|_| Outcome { var: 1e9, loss: 1.0 }).collect();
        let r = assess(&none, 0.95).expect("assess");
        assert!((r.independence - 0.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_unusable_input() {
        assert!(matches!(
            assess(&spaced(50, 10), 0.95),
            Err(BacktestError::TooFewObservations { .. })
        ));
        assert!(matches!(assess(&spaced(200, 10), 0.0), Err(BacktestError::BadConfidence(_))));
        assert!(matches!(assess(&spaced(200, 10), 1.0), Err(BacktestError::BadConfidence(_))));
    }

    #[test]
    fn statistics_match_a_worked_example() {
        // n=250, x=10, p=0.05. Kupiec = -2[ln L(p) - ln L(x/n)]
        //   ln L(0.05) = 240*ln(0.95) + 10*ln(0.05) = -42.267713
        //   ln L(0.04) = 240*ln(0.96) + 10*ln(0.04) = -41.986037
        //   LR = -2 * (-42.267713 + 41.986037)       =   0.563353
        // Cross-checked against an independent computation.
        let mut o: Vec<Outcome> = (0..250).map(|_| Outcome { var: 100.0, loss: 50.0 }).collect();
        for i in 0..10 {
            o[i * 25].loss = 150.0;
        }
        let r = assess(&o, 0.95).expect("assess");
        assert_eq!(r.exceptions, 10);
        assert!((r.kupiec - 0.563_353).abs() < 1e-5, "kupiec {}", r.kupiec);
    }
}
