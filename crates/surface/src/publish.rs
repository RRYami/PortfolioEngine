//! Publishing a fitted surface to Postgres.
//!
//! A run is written whole or not at all, and only becomes visible to the API
//! at the end. `vol.current_run` is a one-row-per-root pointer, so promotion is
//! a single update: the previous run stays intact and is still complete, which
//! is what makes rolling back a bad fit possible. Previously the builder
//! overwrote six files in place and the API read whatever was on disk, so a
//! half-written build was indistinguishable from a good one.

use chrono::NaiveDate;
use sqlx::{Executor, PgPool, QueryBuilder, Postgres, Transaction};
use uuid::Uuid;

use crate::build::{ForwardRow, GridRow, IvRow, SviRow};
use crate::error::SurfaceError;
use crate::factors::FactorFit;

/// Rows per INSERT. Postgres caps a statement at 65535 bound parameters, and
/// the widest table here binds 15 per row, so this leaves generous headroom
/// while keeping the number of round trips down.
const CHUNK: usize = 1000;

fn db(e: impl std::fmt::Display) -> SurfaceError {
    SurfaceError::Database(e.to_string())
}

/// Write every artifact for each fitted root, then promote.
///
/// Returns the run id per root, in the order the fits were given.
pub fn publish(
    dsn: &str,
    forwards: &[ForwardRow],
    ivs: &[IvRow],
    svis: &[SviRow],
    grid: &[GridRow],
    fits: &[FactorFit],
    config: &str,
) -> Result<Vec<(String, Uuid)>, SurfaceError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| SurfaceError::Io("tokio runtime".into(), e))?;

    runtime.block_on(async {
        let pool = PgPool::connect(dsn).await.map_err(db)?;
        let mut out = Vec::new();
        for fit in fits {
            let id = publish_root(&pool, fit, forwards, ivs, svis, grid, config).await?;
            out.push((fit.root.clone(), id));
        }
        Ok(out)
    })
}

async fn publish_root(
    pool: &PgPool,
    fit: &FactorFit,
    forwards: &[ForwardRow],
    ivs: &[IvRow],
    svis: &[SviRow],
    grid: &[GridRow],
    config: &str,
) -> Result<Uuid, SurfaceError> {
    let root = fit.root.as_str();
    let run_id = Uuid::new_v4();
    let first = fit.dates.first().copied().unwrap_or(fit.as_of);
    let last = fit.dates.last().copied().unwrap_or(fit.as_of);

    let mut tx = pool.begin().await.map_err(db)?;

    sqlx::query(
        "INSERT INTO vol.surface_run
           (run_id, root, as_of, first_session, last_session, components,
            git_sha, config)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)",
    )
    .bind(run_id)
    .bind(root)
    .bind(fit.as_of)
    .bind(first)
    .bind(last)
    .bind(i32::try_from(fit.fit.components()).unwrap_or(0))
    .bind(git_sha())
    .bind(config)
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    insert_forwards(&mut tx, run_id, root, forwards).await?;
    insert_ivs(&mut tx, run_id, root, ivs).await?;
    insert_svis(&mut tx, run_id, root, svis).await?;
    insert_grid(&mut tx, run_id, root, grid).await?;
    insert_loadings(&mut tx, run_id, fit).await?;
    insert_scores(&mut tx, run_id, fit).await?;

    // Promotion is last and inside the transaction: until it lands the run is
    // invisible to readers, and if anything above failed nothing moved.
    sqlx::query(
        "INSERT INTO vol.current_run (root, run_id, promoted_at)
         VALUES ($1, $2, now())
         ON CONFLICT (root) DO UPDATE
           SET run_id = EXCLUDED.run_id, promoted_at = now()",
    )
    .bind(root)
    .bind(run_id)
    .execute(&mut *tx)
    .await
    .map_err(db)?;

    tx.commit().await.map_err(db)?;
    Ok(run_id)
}

/// The commit the binary was built from, if the build recorded one.
fn git_sha() -> Option<String> {
    option_env!("PTF_GIT_SHA").map(String::from)
}

async fn insert_forwards(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    root: &str,
    rows: &[ForwardRow],
) -> Result<(), SurfaceError> {
    for chunk in rows.iter().filter(|r| r.root == root).collect::<Vec<_>>().chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO vol.forward_curve (run_id, quote_date, root, expiry, tte, \
             forward, discount, pairs_used, rmse, curve_rate, curve_expiries, \
             curve_clamped) ",
        );
        qb.push_values(chunk, |mut b, r| {
            b.push_bind(run_id)
                .push_bind(r.quote_date)
                .push_bind(&r.root)
                .push_bind(r.expiry)
                .push_bind(r.tte)
                .push_bind(r.forward)
                .push_bind(r.discount)
                .push_bind(i32::try_from(r.pairs_used).unwrap_or(0))
                .push_bind(r.rmse)
                .push_bind(r.curve_rate)
                .push_bind(i32::try_from(r.curve_expiries).unwrap_or(0))
                .push_bind(r.curve_clamped);
        });
        tx.execute(qb.build()).await.map_err(db)?;
    }
    Ok(())
}

async fn insert_ivs(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    root: &str,
    rows: &[IvRow],
) -> Result<(), SurfaceError> {
    for chunk in rows.iter().filter(|r| r.root == root).collect::<Vec<_>>().chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO vol.iv_quote (run_id, quote_date, root, expiry, opt_right, \
             strike, mid, tte, log_moneyness, iv, vega, forward, rel_spread, size, \
             stale) ",
        );
        qb.push_values(chunk, |mut b, r| {
            b.push_bind(run_id)
                .push_bind(r.quote_date)
                .push_bind(&r.root)
                .push_bind(r.expiry)
                .push_bind(right_char(r.opt_right))
                .push_bind(r.strike)
                .push_bind(r.mid)
                .push_bind(r.tte)
                .push_bind(r.log_moneyness)
                .push_bind(r.iv)
                .push_bind(r.vega)
                .push_bind(r.forward)
                .push_bind(nan_to_none(r.rel_spread))
                .push_bind(r.size)
                .push_bind(r.stale);
        });
        tx.execute(qb.build()).await.map_err(db)?;
    }
    Ok(())
}

async fn insert_svis(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    root: &str,
    rows: &[SviRow],
) -> Result<(), SurfaceError> {
    for chunk in rows.iter().filter(|r| r.root == root).collect::<Vec<_>>().chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO vol.svi_slice (run_id, quote_date, root, expiry, tte, a, b, \
             rho, m, sigma, rmse_vol, points, min_durrleman, k_lo, k_hi) ",
        );
        qb.push_values(chunk, |mut bind, r| {
            bind.push_bind(run_id)
                .push_bind(r.quote_date)
                .push_bind(&r.root)
                .push_bind(r.expiry)
                .push_bind(r.tte)
                .push_bind(r.params.a)
                .push_bind(r.params.b)
                .push_bind(r.params.rho)
                .push_bind(r.params.m)
                .push_bind(r.params.sigma)
                .push_bind(r.rmse_vol)
                .push_bind(i32::try_from(r.points).unwrap_or(0))
                .push_bind(r.min_durrleman)
                .push_bind(r.k_lo)
                .push_bind(r.k_hi);
        });
        tx.execute(qb.build()).await.map_err(db)?;
    }
    Ok(())
}

async fn insert_grid(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    root: &str,
    rows: &[GridRow],
) -> Result<(), SurfaceError> {
    for chunk in rows.iter().filter(|r| r.root == root).collect::<Vec<_>>().chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO vol.grid_cell (run_id, quote_date, root, z, tte, k, \
             total_variance, vol, extrapolated) ",
        );
        qb.push_values(chunk, |mut b, r| {
            b.push_bind(run_id)
                .push_bind(r.quote_date)
                .push_bind(&r.root)
                .push_bind(r.z)
                .push_bind(r.tte)
                .push_bind(r.k)
                .push_bind(r.total_variance)
                .push_bind(r.vol)
                .push_bind(r.extrapolated);
        });
        tx.execute(qb.build()).await.map_err(db)?;
    }
    Ok(())
}

async fn insert_loadings(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    fit: &FactorFit,
) -> Result<(), SurfaceError> {
    let k = fit.fit.components();
    let mut qb = QueryBuilder::new(
        "INSERT INTO vol.pca_loading (run_id, pc, z, tte, loading, cell_mean, \
         cell_sd, explained) ",
    );
    let mut rows = Vec::new();
    for pc in 0..k {
        for (i, cell) in fit.cells.iter().enumerate() {
            rows.push((
                i32::try_from(pc + 1).unwrap_or(0),
                cell.z,
                cell.tte,
                fit.fit.loadings[pc][i],
                fit.fit.mean[i],
                fit.fit.sd[i],
                fit.fit.explained.get(pc).copied().unwrap_or(0.0),
            ));
        }
    }
    if rows.is_empty() {
        return Ok(());
    }
    qb.push_values(&rows, |mut b, r| {
        b.push_bind(run_id)
            .push_bind(r.0)
            .push_bind(r.1)
            .push_bind(r.2)
            .push_bind(r.3)
            .push_bind(r.4)
            .push_bind(r.5)
            .push_bind(r.6);
    });
    tx.execute(qb.build()).await.map_err(db)?;
    Ok(())
}

async fn insert_scores(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    fit: &FactorFit,
) -> Result<(), SurfaceError> {
    let k = fit.fit.components();
    let mut rows: Vec<(NaiveDate, i32, f64)> = Vec::new();
    for (row, date) in fit.fit.scores.iter().zip(fit.dates.iter()) {
        for (pc, score) in row.iter().take(k).enumerate() {
            rows.push((*date, i32::try_from(pc + 1).unwrap_or(0), *score));
        }
    }
    for chunk in rows.chunks(CHUNK) {
        let mut qb =
            QueryBuilder::new("INSERT INTO vol.pca_score (run_id, quote_date, pc, score) ");
        qb.push_values(chunk, |mut b, r| {
            b.push_bind(run_id).push_bind(r.0).push_bind(r.1).push_bind(r.2);
        });
        tx.execute(qb.build()).await.map_err(db)?;
    }
    Ok(())
}

fn right_char(r: ptf_engine::vol::OptionRight) -> &'static str {
    match r {
        ptf_engine::vol::OptionRight::Call => "C",
        ptf_engine::vol::OptionRight::Put => "P",
    }
}

/// `NaN` is how the pipeline spells "no quoted spread"; SQL spells it NULL.
/// Postgres accepts NaN in a double column, but it would then compare unequal
/// to itself and quietly poison any aggregate over the column.
fn nan_to_none(x: f64) -> Option<f64> {
    if x.is_nan() { None } else { Some(x) }
}
