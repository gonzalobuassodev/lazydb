# CI Build Optimization Implementation Plan

**Goal:** Reduce repeated DuckDB compilation and preserve the existing test and release compatibility gates.

**Architecture:** Pin the effective CI toolchain through `RUSTUP_TOOLCHAIN`, align development and test debug settings in CI, and cache dependency artifacts with the existing pinned Rust cache action. Keep caches separate for each runner environment and run Linux release cache metadata commands inside the same pinned native container as the build.

**Tech Stack:** GitHub Actions, Rust 1.94.0, Swatinem/rust-cache, Docker, POSIX shell, Python unittest.

## Task 1: Ordinary CI

- Modify `.github/workflows/ci.yml` to cancel superseded runs on the same branch or pull request.
- Set `RUSTUP_TOOLCHAIN=1.94.0`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `CARGO_BUILD_JOBS=2` in the workflow environment.
- Run formatting before expensive cache restoration and compilation; remove the redundant Linux test prebuild from the ordinary Rust job.
- Cache dependency build artifacts with the existing pinned Rust cache action. Let the main-branch Rust job populate the Ubuntu 24.04 cache, let database tests restore it, and give MongoDB its own Ubuntu 22.04 cache.
- Keep macOS dependency checks on the same features and profile as the tests.

## Task 2: Linux release cache consistency

- Modify `scripts/release/build-linux.sh` to support preparing the image and executing metadata commands without compiling the application.
- Use identical repository and Cargo-home paths inside and outside the container so cache cleanup can associate metadata paths with host files.
- Pin the container toolchain explicitly and retain native architecture, image digest, user ownership, and build exit-status checks.
- Modify `.github/workflows/release.yml` to prepare the image before restoring the Linux cache, query cache metadata through the container entry point, and key caches on image and build-script changes.
- Reuse the normal Ubuntu 24.04 quality cache; preserve all release smoke, dependency, archive, and runtime-policy checks.

## Task 3: Validation and operating guidance

- Add behavioral regression coverage for the new Linux wrapper modes, cache paths, environment, and error propagation using an isolated fake Docker executable.
- Include that suite in `scripts/release/test-distribution.sh`.
- Document cache ownership, capacity, cold/warm verification, and the deferred conditional use of sccache in `docs/ci.md`.
- Run workflow validation with actionlint, shell validation, distribution contracts, and a minimal Rust 1.94.0 profile-reuse probe outside the repository.
- Do not push, publish, delete remote caches, or trigger a release while validating these local changes. Measure actual cloud performance after the reviewed changes reach GitHub.

## Implementation and validation results

All three tasks are implemented. Removed the unused Release-only `RUST_VERSION`
variable so it does not split the shared CI cache's environment hash. Release
timing artifacts use `build-timings-*`, outside the existing release-asset
download patterns. The container mounts a writable Cargo home at its host path
so metadata paths and Cargo's cache locks work together.

Local validation completed on 2026-10-21:

- actionlint 1.7.12 with ShellCheck 0.11.0 passed for both modified workflows.
- ShellCheck and POSIX shell syntax validation passed for the modified scripts.
- All 10 new wrapper behavior tests passed, including with the system Python 3.9.
- The full distribution suite passed with Python 3.13; its existing native ELF
  integration test was skipped on macOS. CI still requires that test on Linux.
- `cargo +1.94.0 fmt --all -- --check` and `git diff --check` passed.
- An isolated Rust 1.94.0 dependency probe executed its native build script once
  across Clippy, tests, and a development build with the unified CI settings.
- Compared workflow definitions against HEAD: existing jobs, Rust/release
  platforms, database services/test targets, and Release concurrency are retained.
  The shared Ubuntu cache families and Cargo/Rust environments match, and timing
  artifacts cannot match the release download patterns.

The local Docker daemon is unavailable, so real Linux container builds and
runtime checks remain for native Linux CI. No full application Clippy/test build
or cloud cache/performance run was performed. The previously observed application
Clippy failure (`src/app.rs` redundant `staged` and `staged_groups` locals) is
outside these build-configuration changes and still needs a separate fix.
