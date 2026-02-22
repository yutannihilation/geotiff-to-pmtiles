# Repository Guidelines

## Project Structure & Module Organization
This repository is a Rust binary crate.

- `Cargo.toml`: package metadata and dependencies.
- `src/main.rs`: CLI entry point.
- `src/`: add new modules as the project grows (for example `src/conversion.rs`, `src/io.rs`).
- `tests/`: integration tests (create this directory when adding end-to-end behavior tests).

Keep conversion logic out of `main.rs`; use modules in `src/` and call them from `main`.

## Build, Test, and Development Commands
Use standard Cargo commands from the repository root:

- `cargo check`: fast type-check without producing a binary.
- `cargo run`: build and run the executable locally.
- `cargo test`: run unit and integration tests.
- `cargo fmt`: apply Rust formatting.
- `cargo clippy -- -D warnings`: lint and fail on warnings.
- `cargo build --release`: produce an optimized binary for distribution.

Run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` before opening a PR.

## Coding Style & Naming Conventions
Follow idiomatic Rust style:

- Formatting is enforced with `rustfmt` (`cargo fmt`).
- Use `snake_case` for functions/modules/files, `PascalCase` for types/traits, and `SCREAMING_SNAKE_CASE` for constants.
- Prefer small, single-purpose functions and explicit error handling with `Result`.
- Add brief doc comments (`///`) for public APIs and non-obvious behavior.

## Testing Guidelines
Testing framework: Rust built-in test harness.

- Unit tests should live next to implementation in `#[cfg(test)] mod tests`.
- Integration tests should live under `tests/` with descriptive names (example: `tests/convert_geotiff.rs`).
- Name tests by behavior (example: `converts_small_raster_to_single_tile`).

For this early-stage repo, target meaningful coverage of all conversion and I/O branches as features are added.

## Commit & Pull Request Guidelines
No historical convention exists yet; use this default:

- Commit format: `type(scope): short summary` (example: `feat(conversion): add tile pyramid builder`).
- Keep commits focused and atomic.
- PRs should include: purpose, key changes, test evidence (`cargo test` output summary), and linked issues.
- For CLI/output changes, include a short example command and resulting output.
