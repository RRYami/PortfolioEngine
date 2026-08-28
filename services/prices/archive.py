"""Build a verified, portable archive of the data that cannot be re-derived.

Most of what lives in `data/` is disposable. The fitted surfaces, the IV table,
the forwards and the curated quote table are all functions of two inputs, and
rerunning the pipeline reproduces them exactly. Three things are not:

  * `raw_quotes/` -- vendor snapshots that cost money. Re-acquiring them means
    paying for them again.
  * `option_ingest_log` -- which sessions were bought and which came back empty
    because the market was shut. `raw_quotes/` has no directory for a holiday,
    so without the log an absent session is ambiguous: not bought, or nothing
    to buy? That ambiguity is what aborts a resumed ingest.
  * equity closes and FX -- free to refetch, but yfinance adjusts history
    retroactively for splits and dividends, so a refetch next year returns
    different numbers. Cheap to replace, impossible to reproduce.

The archive is those three, as plain parquet with real types and no identifiers
private to this repository, so another project can read it with no knowledge of
this one.

Nothing reads the archive at runtime. That is deliberate: an export that sits in
the serving path is an export whose schema can drift into live numbers, which is
exactly how a dropped date column once turned every correlation to noise. This
one is verified against its source when written and otherwise left alone.

Usage:
    python archive.py                  # build into data/archive/<today>/
    python archive.py --verify PATH    # recheck an existing archive
    python archive.py --freeze         # also make raw_quotes/ read-only
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import stat
import subprocess
import sys
from pathlib import Path

import duckdb

DATA = Path("data")
RAW = DATA / "raw_quotes"
OPTIONS_DB = DATA / "options.duckdb"
PRICES_DB = DATA / "prices.duckdb"

# The 16 real columns of a raw snapshot. `date` is *not* among them: DuckDB
# synthesises it from the `date=` path when hive partitioning is on, and letting
# it through would bake a duplicate of `quote_date` into the archive.
QUOTE_COLUMNS = [
    "quote_date", "root", "expiry", "opt_right", "strike",
    "instrument_id", "raw_symbol", "bid", "ask", "mid",
    "rel_spread", "tte", "snapshot_ts", "bid_size", "ask_size",
    "last_update_ts",
]

# Ordered by the natural key so the bytes are reproducible: the same tree
# archived twice must produce the same checksum, or the checksum says nothing.
QUOTE_ORDER = "quote_date, root, expiry, opt_right, strike"

# Aggregates compared between the tree and the consolidated file. Row counts
# alone would pass even if every price were zeroed.
FINGERPRINT = """
SELECT count(*)                        AS rows,
       count(DISTINCT quote_date)      AS sessions,
       count(DISTINCT raw_symbol)      AS contracts,
       round(sum(bid), 4)              AS sum_bid,
       round(sum(ask), 4)              AS sum_ask,
       round(sum(mid), 4)              AS sum_mid,
       sum(bid_size)                   AS sum_bid_size,
       sum(ask_size)                   AS sum_ask_size,
       min(quote_date)::VARCHAR        AS first_session,
       max(quote_date)::VARCHAR        AS last_session,
       max(snapshot_ts)::VARCHAR       AS last_snapshot
FROM {src}
"""


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def git_sha() -> str | None:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True, text=True, check=True,
        )
        return out.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def tree_source() -> str:
    """The raw tree as a DuckDB relation, with path inference off."""
    return (
        f"read_parquet('{RAW}/*/*.parquet', hive_partitioning=0)"
    )


def fingerprint(con, src: str) -> dict:
    row = con.execute(FINGERPRINT.format(src=src)).fetchone()
    cols = [d[0] for d in con.description]
    return dict(zip(cols, row))


def describe(con, src: str) -> list[dict]:
    rows = con.execute(f"DESCRIBE SELECT * FROM {src}").fetchall()
    return [{"name": r[0], "type": r[1]} for r in rows]


def raw_inventory() -> list[dict]:
    """Per-file checksums, so bit-rot in the paid data is detectable later."""
    out = []
    for path in sorted(RAW.glob("*/*.parquet")):
        out.append({
            "path": str(path.relative_to(DATA)),
            "bytes": path.stat().st_size,
            "sha256": sha256(path),
        })
    return out


def build(out_dir: Path, freeze: bool) -> int:
    if not RAW.exists():
        print(f"missing {RAW}", file=sys.stderr)
        return 1
    out_dir.mkdir(parents=True, exist_ok=True)
    con = duckdb.connect(":memory:")

    src = tree_source()
    before = fingerprint(con, src)
    print(f"raw tree: {before['rows']:,} rows, {before['sessions']} sessions, "
          f"{before['first_session']}..{before['last_session']}")

    # 1. Consolidate the tree. Explicit column list rather than * so a stray
    #    path-derived column cannot slip in.
    quotes = out_dir / "option_quotes.parquet"
    cols = ", ".join(QUOTE_COLUMNS)
    con.execute(
        f"COPY (SELECT {cols} FROM {src} ORDER BY {QUOTE_ORDER}) "
        f"TO '{quotes}' (FORMAT PARQUET, COMPRESSION ZSTD)"
    )

    # 2. Verify the consolidated file against the tree it came from. This is
    #    the whole point of the exercise; everything else is bookkeeping.
    after = fingerprint(con, f"read_parquet('{quotes}')")
    mismatches = {k: (before[k], after[k]) for k in before if before[k] != after[k]}
    if mismatches:
        print("VERIFY FAILED — consolidated file differs from the tree:", file=sys.stderr)
        for k, (a, b) in mismatches.items():
            print(f"  {k}: tree={a!r} archive={b!r}", file=sys.stderr)
        return 1

    schema = describe(con, f"read_parquet('{quotes}')")
    got = [c["name"] for c in schema]
    if got != QUOTE_COLUMNS:
        print(f"VERIFY FAILED — column set changed:\n  want {QUOTE_COLUMNS}\n  got  {got}",
              file=sys.stderr)
        return 1
    print(f"verified: consolidated file matches the tree on {len(before)} aggregates")

    artifacts = [{
        "name": "option_quotes",
        "file": quotes.name,
        "source": "raw_quotes/ (vendor snapshots)",
        "derivable": False,
        "rows": before["rows"],
        "columns": schema,
        "fingerprint": before,
    }]

    # 3. The ingest log: small, and the only record of what was paid for.
    if OPTIONS_DB.exists():
        con.execute(f"ATTACH '{OPTIONS_DB}' AS o (READ_ONLY)")
        log = out_dir / "option_ingest_log.parquet"
        con.execute(
            f"COPY (SELECT * FROM o.option_ingest_log ORDER BY quote_date, root) "
            f"TO '{log}' (FORMAT PARQUET, COMPRESSION ZSTD)"
        )
        n = con.execute(f"SELECT count(*) FROM read_parquet('{log}')").fetchone()[0]
        by_status = dict(con.execute(
            "SELECT status, count(*) FROM o.option_ingest_log GROUP BY 1"
        ).fetchall())
        artifacts.append({
            "name": "option_ingest_log",
            "file": log.name,
            "source": "options.duckdb",
            "derivable": False,
            "rows": n,
            "columns": describe(con, f"read_parquet('{log}')"),
            "fingerprint": {"rows": n, "by_status": by_status},
        })
        print(f"ingest log: {n} rows {by_status}")

    # 4. Equity closes and FX, with real DATE columns. The parquet the service
    #    exports stringifies them, which every downstream reader then reparses.
    if PRICES_DB.exists():
        con.execute(f"ATTACH '{PRICES_DB}' AS p (READ_ONLY)")
        for table, fname, order in [
            ("prices", "equity_close.parquet", "symbol, date"),
            ("fx", "fx_rate.parquet", "ccy, date"),
        ]:
            dest = out_dir / fname
            con.execute(
                f"COPY (SELECT * FROM p.{table} ORDER BY {order}) "
                f"TO '{dest}' (FORMAT PARQUET, COMPRESSION ZSTD)"
            )
            n = con.execute(f"SELECT count(*) FROM read_parquet('{dest}')").fetchone()[0]
            span = con.execute(
                f"SELECT min(date)::VARCHAR, max(date)::VARCHAR FROM read_parquet('{dest}')"
            ).fetchone()
            artifacts.append({
                "name": table,
                "file": fname,
                "source": "prices.duckdb (yfinance, adjusted)",
                # Refetchable, but not reproducible: adjustment factors move.
                "derivable": False,
                "rows": n,
                "columns": describe(con, f"read_parquet('{dest}')"),
                "fingerprint": {"rows": n, "first": span[0], "last": span[1]},
            })
            print(f"{table}: {n:,} rows, {span[0]}..{span[1]}")

    for a in artifacts:
        path = out_dir / a["file"]
        a["bytes"] = path.stat().st_size
        a["sha256"] = sha256(path)

    print("checksumming raw tree…")
    inventory = raw_inventory()

    manifest = {
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_sha": git_sha(),
        "generator": "services/prices/archive.py",
        "artifacts": artifacts,
        "raw_quotes": {
            "files": len(inventory),
            "bytes": sum(f["bytes"] for f in inventory),
            "inventory": inventory,
        },
    }
    (out_dir / "MANIFEST.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (out_dir / "README.md").write_text(readme(manifest))

    if freeze:
        n = 0
        for path in RAW.rglob("*"):
            mode = path.stat().st_mode
            path.chmod(mode & ~(stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))
            n += 1
        print(f"froze {n} paths under {RAW} (read-only)")

    total = sum(a["bytes"] for a in artifacts)
    print(f"\narchive written to {out_dir}  ({total/1e6:.1f} MB + manifest)")
    return 0


def readme(manifest: dict) -> str:
    rows = "\n".join(
        f"| `{a['file']}` | {a['rows']:,} | {a['bytes']/1e6:.1f} MB | {a['source']} |"
        for a in manifest["artifacts"]
    )
    return f"""# Option and price archive

Created {manifest['created_at']} from commit `{manifest['git_sha']}`.

Self-contained. Reading this needs nothing from the project that produced it:
every table is keyed by ticker and date, with no internal identifiers.

| file | rows | size | source |
|---|---|---|---|
{rows}

`option_quotes.parquet` is the consolidation of the per-session vendor
snapshots, verified against them on row count, session count, contract count,
the sum of every price and size column, and the last snapshot timestamp.
`MANIFEST.json` carries those figures plus a SHA-256 for every source file.

`option_ingest_log.parquet` distinguishes a session that returned nothing
because the market was shut from one that was never fetched. Absence in
`option_quotes` is ambiguous without it.

## Reading it

```python
import duckdb
q = duckdb.sql("SELECT * FROM 'option_quotes.parquet' WHERE quote_date = '2026-08-20'")
```

```python
import pandas as pd
df = pd.read_parquet("option_quotes.parquet")
```

## Verifying it

```bash
python archive.py --verify .
```
"""


def verify(out_dir: Path) -> int:
    path = out_dir / "MANIFEST.json"
    if not path.exists():
        print(f"no manifest at {path}", file=sys.stderr)
        return 1
    manifest = json.loads(path.read_text())
    bad = 0
    for a in manifest["artifacts"]:
        f = out_dir / a["file"]
        if not f.exists():
            print(f"MISSING {a['file']}", file=sys.stderr)
            bad += 1
            continue
        digest = sha256(f)
        ok = digest == a["sha256"]
        print(f"  {'ok  ' if ok else 'BAD '} {a['file']}  {a['rows']:,} rows")
        if not ok:
            print(f"       want {a['sha256']}\n       got  {digest}", file=sys.stderr)
            bad += 1

    # The raw tree is checked separately: it is the thing the rest is rebuilt
    # from, so silent corruption there is the expensive kind.
    inv = manifest.get("raw_quotes", {}).get("inventory", [])
    drift = 0
    for entry in inv:
        f = DATA / entry["path"]
        if not f.exists() or sha256(f) != entry["sha256"]:
            print(f"  BAD  {entry['path']}", file=sys.stderr)
            drift += 1
    print(f"  raw tree: {len(inv) - drift}/{len(inv)} files intact")
    bad += drift

    print("\nverify: FAILED" if bad else "\nverify: all artifacts intact")
    return 1 if bad else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", default=None, help="archive directory (default data/archive/<today>)")
    ap.add_argument("--verify", metavar="PATH", help="recheck an existing archive and exit")
    ap.add_argument("--freeze", action="store_true", help="make raw_quotes/ read-only after writing")
    args = ap.parse_args()

    if args.verify:
        return verify(Path(args.verify))

    out = Path(args.out) if args.out else DATA / "archive" / dt.date.today().isoformat()
    return build(out, args.freeze)


if __name__ == "__main__":
    raise SystemExit(main())
