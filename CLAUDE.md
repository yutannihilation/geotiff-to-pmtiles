# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Rust CLI tool that converts GeoTIFF files to PMTiles with AVIF image tiles. Single statically linked binary, supports multiple inputs without pre-merge, outputs 256×256 AVIF tiles.

## Build & Development Commands

```bash
cargo check                        # Type-check without building
cargo build                        # Debug build
cargo build --release              # Optimized release build (thin LTO)
cargo test                         # Run all tests
cargo test <test_name>             # Run a single test by name
cargo fmt                          # Format code
cargo clippy -- -D warnings        # Lint (warnings are errors)
```

NASM is required for AVIF encoding (installed per-platform in CI).

Generate benchmark fixtures: `cargo run --manifest-path tools/generate-bench-data/Cargo.toml`

Run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` before opening a PR.

## Architecture

**Pipeline** (see `docs/ALGORITHMS.md` for details):
1. CLI parsing (`src/cli.rs`) → expand globs (`src/resample/inputs.rs`)
2. Load metadata + georeferencing for each source (`src/resample/georef.rs`)
3. Reproject source corners to Web Mercator (EPSG:3857) via `proj-lite`
4. Compute tile coverage and zoom levels (`src/resample/types.rs`)
5. Parallel tile rendering with rayon (`src/convert/mod.rs`)
6. Encode tiles to AVIF (`src/resample/render.rs`) → write PMTiles

**Key modules:**

- `src/convert/mod.rs` — Core conversion orchestrator. Schedules tile jobs in Morton-curve order for cache locality, renders in parallel with per-worker state, writes in tile_id order for PMTiles clustering.
- `src/convert/cache.rs` — `GlobalChunkCache`: byte-bounded LRU cache for decoded TIFF chunks shared across workers (default 128 MiB).
- `src/convert/source.rs` — `ChunkedTiffSampler`: lazy on-demand TIFF chunk decoding with spatial prefetching. Falls back to full raster loading for complex layouts.
- `src/convert/render.rs` — Tile pixel rendering with nearest/bilinear sampling across multiple sources. Uses per-row affine stepping.
- `src/resample/types.rs` — Core data types: `GeoTransform` (tiepoint or affine), `Georef`, `SourceMetadata`, `NoDataSpec`, tile math functions.
- `src/resample/georef.rs` — Georeferencing: reads TIFF tags (ModelTransformationTag → ModelPixelScale+Tiepoint → world file fallback), EPSG extraction, projection.

**Parallelism pattern:** Rayon thread pool with per-worker `WorkerState` (owns decoders + cache ref) to avoid lock contention. Render order uses Morton curve for spatial locality; write order uses ascending tile_id.

**Dual sampling paths:** Chunked TIFF (efficient, on-demand) vs. full raster (fallback for unsupported layouts).

## Conventions

- Commit format: `type(scope): summary` (e.g. `feat(conversion): add tile pyramid builder`)
- Unit tests live in `#[cfg(test)] mod tests` next to the implementation
- Error handling: `Result<T, Box<dyn std::error::Error>>`
- Constants in `src/resample/mod.rs`: `TILE_SIZE=256`, `DEFAULT_AVIF_QUALITY=55`, `DEFAULT_AVIF_SPEED=4`
