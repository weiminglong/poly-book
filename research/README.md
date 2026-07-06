# Research access

This directory demonstrates that poly-book's Parquet output is directly usable
as a research dataset, with no poly-book code in the loop.

## Contents

- [`orderbook_analysis.ipynb`](orderbook_analysis.ipynb) — an executed
  notebook (outputs and plots embedded, so it renders on GitHub) computing
  microstructure analytics over the committed sample capture at
  [`demo/data/`](../demo/data/):
  - dataset layout, fixed-point encoding, and event counts
  - top-of-book, mid, and spread time series derived from venue snapshots
  - microprice (size-weighted mid) vs mid, and its short-horizon predictive skew
  - multi-level order-flow imbalance vs contemporaneous mid moves
    (Cont–Kukanov–Stoikov)
  - trade flow, trade signs, effective and realized spreads
  - binary-outcome specifics: cross-token complementarity and terminal pinning

## Running the notebook

The notebook needs only `polars`, `pyarrow`, `matplotlib`, and a Jupyter
front-end. With [uv](https://docs.astral.sh/uv/) (no environment setup):

```bash
uv run --with polars,pyarrow,matplotlib,jupyter jupyter lab research/
```

Or re-execute it headlessly from the repository root:

```bash
uv run --with polars,pyarrow,matplotlib,jupyter \
  jupyter nbconvert --to notebook --execute --inplace research/orderbook_analysis.ipynb
```

Without uv, any Python 3.10+ environment with those four packages installed
works the same way. Full execution takes well under a minute; the notebook
reads `../demo/data` relative to its own directory (falling back to
`demo/data` when run from the repository root).

## Data provenance

`demo/data/` is a real capture, recorded with `poly-book auto-ingest` on
2026-07-06 (UTC): the complete life of one Polymarket BTC 5-minute up/down
market — two complementary outcome tokens, roughly 536k events over five
minutes — in the same hour-partitioned split-Parquet layout
(`book_events/`, `trade_events/`, `ingest_events/`, `book_checkpoints/`) the
live pipeline writes. See [`demo/data/README.md`](../demo/data/README.md) for
how to regenerate a capture of your own, and `docs/operations.md` for the
schema and partition layout. The same capture powers `poly-book demo` and the
API examples in [`examples/api-cookbook.md`](../examples/api-cookbook.md).
