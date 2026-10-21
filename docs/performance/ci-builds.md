# CI build measurements

Measurements below were collected from GitHub-hosted runners on 2026-10-21.
The initial optimized build uses commit
[`10015e5`](https://github.com/yelog/lazydb/commit/10015e53f0183cf18e94cc0aa5073ea7813a6aae),
including the CI changes in `144c751` and removal of two redundant application
bindings that had blocked Clippy. The follow-up cache-key fix is
[`2e2e8a0`](https://github.com/yelog/lazydb/commit/2e2e8a0afbd0707a91c44e02a3d3f2ad301ac367).

With the final configuration, a cold run took 26m 19s and a fully warm run took
16m 18s, compared with the earlier successful baseline of 79m 23s. The warm run
reduced elapsed time by 79.5%; all seven jobs passed.

## Full CI with a cold dependency cache

| Job | Previous successful run | Optimized cold run |
| --- | ---: | ---: |
| Entire workflow | 79m 23s | 27m 00s |
| Rust, Ubuntu 24.04 | 79m 20s | 23m 45s |
| Rust, macOS 14 | 54m 02s | 26m 56s |
| PostgreSQL/MySQL/SQL Server and other adapters | 15m 18s | 18m 03s |
| MongoDB integration | 15m 40s | 16m 15s |

Sources:

- [Previous successful CI, run 39567901018](https://github.com/yelog/lazydb/actions/runs/39567901018),
  commit `9b0c182`.
- [Optimized cold CI, run 39606567676](https://github.com/yelog/lazydb/actions/runs/39606567676),
  commit `10015e5`.

All seven optimized CI jobs passed, including distribution contracts, real Linux
ELF inspection/strip, both Windows installer hosts, Clippy, macOS system-library
checks, and the existing database integrations. Each Rust job reported 3328
passed tests and 12 ignored tests. The database job reported 132 passed tests;
MongoDB reported 20. The earlier baseline reported 3324 passed tests in each
Rust job and the same integration-test totals.

The workflow elapsed time fell by 66.0% in this comparison. This is one pair of
runs with different commits, rather than a controlled benchmark. Runner
variation still affects individual jobs; the cold database jobs did not become
faster. Job durations include setup and cache operations. Whole-workflow elapsed
time is measured from creation to completion, so it also includes initial queue
time.

Cargo timing artifacts confirm that the expensive native compilation was reused
between Clippy and tests:

| Platform | DuckDB native build script in Clippy | DuckDB native build script in tests | DuckDB Rust library compilation in tests |
| --- | ---: | --- | ---: |
| Ubuntu 24.04 | 756.23s | Not executed | 0.68s |
| macOS 14 | 534.49s | Not executed | 1.70s |

`Compiling libduckdb-sys` still appears in both command logs because Cargo checks
the Rust library for Clippy and compiles it for tests. Counting those messages
alone cannot determine whether the C++ build repeated. The timing reports show
one `run-custom-build` unit for DuckDB per Rust job, in Clippy only.

The cold run saved three main-branch dependency caches: Ubuntu 24.04
(883,986,010 bytes), macOS 14 (844,698,935 bytes), and Ubuntu 22.04/MongoDB
(789,225,839 bytes). Ubuntu Rust and the database reader printed the same cache
key. No manual cache removal was performed.

## First cache-reuse run

The [second full CI run, 39610636226](https://github.com/yelog/lazydb/actions/runs/39610636226)
tested the same `10015e5` commit after the first run had saved its caches. All
seven jobs passed, with the same test totals as the cold run.

| Job | Optimized cold run | First cache-reuse run | Cache result |
| --- | ---: | ---: | --- |
| Entire workflow | 27m 00s | 19m 34s | Mixed |
| Rust, Ubuntu 24.04 | 23m 45s | 19m 28s | Miss due to unused runner compiler |
| Rust, macOS 14 | 26m 56s | 17m 10s | Exact hit |
| Database adapters | 18m 03s | 4m 59s | Exact hit |
| MongoDB integration | 16m 15s | 2m 08s | Exact hit |

macOS, the database job, and MongoDB did not recompile DuckDB. The database job
restored the cache written by the Ubuntu Rust job, demonstrating cross-job
artifact reuse. The macOS test step still took 13m 14s because workspace test
binaries are excluded from the dependency cache; an exact dependency cache hit
does not eliminate application compilation and linking.

The Ubuntu Rust job used the pinned Rust 1.94.0 compiler, but its runner also had
unused Rust 1.99.0 installed. The first run and the database job had unused
1.99.1 instead. The action hashed those extra compilers, changing the environment
hash from `bb817241` to `f4d0fca4` and causing a cold rebuild. Thus this run is
not evidence of a fully warm workflow, despite its 75.4% reduction from the
earlier 79m 23s baseline.

The follow-up fix uses `scripts/ci/rust-cache-command.sh` for every host cache
writer/reader. It enumerates only `RUSTUP_TOOLCHAIN` for the cache action's
compiler hash and forwards all other commands unchanged. Native Linux release
containers already have only the pinned toolchain and retain their existing
command wrapper.

## Cache keys after compiler enumeration was fixed

The [first run of the fixed cache configuration, 39614440270](https://github.com/yelog/lazydb/actions/runs/39614440270)
completed with a successful workflow conclusion in 26m 19s. The host cache
action reported only Rust 1.94.0 in every job. The Ubuntu Rust writer and database
reader both printed `v0-rust-ci-ubuntu-24.04-Linux-x64-b47472da-7981e2a3`.

This was a cold run for the new key format, so it rebuilt dependencies once and
saved the new caches. Ubuntu and macOS Rust jobs finished in 19m 34s and 26m 15s;
MongoDB finished in 13m 20s. Timing reports again showed DuckDB's native build
script only in Clippy, taking 600.34s on Ubuntu and 458.71s on macOS, with no
native rebuild during the test step.

The GitHub job API retained an `in_progress` status for the database job even
after the workflow completed successfully. Its downloaded log and step data show
every test and cleanup step passed, ending at 14:28:51 UTC (18m 49s after that
job started). That stale job status is not used as a completed-job timing in
the comparisons above.

## Full CI with warm dependency caches

The [final warm run, 39618588158](https://github.com/yelog/lazydb/actions/runs/39618588158)
tested the same `2e2e8a0` commit after run `39614440270` had saved the fixed-key
caches. All seven jobs completed successfully, and all four dependency-cache
readers reported `full match: true`.

| Job | Previous successful run | Fully warm run | Cache result |
| --- | ---: | ---: | --- |
| Entire workflow | 79m 23s | 16m 18s | All dependency caches hit |
| Rust, Ubuntu 24.04 | 79m 20s | 6m 58s | Exact hit |
| Rust, macOS 14 | 54m 02s | 16m 12s | Exact hit |
| Database adapters | 15m 18s | 4m 46s | Exact hit; shared Ubuntu Rust cache |
| MongoDB integration | 15m 40s | 2m 01s | Exact hit |

The workflow took 63m 05s less than the baseline, a 79.5% reduction. It was also
38.1% faster than the fixed-key cold run. These are individual observations on
GitHub-hosted runners, not a guarantee of future latency. The baseline and final
configuration use different commits; the fixed-key cold and warm runs use the
same commit and test suites.

| Cache reader | Restored archive size | Restore step | Remaining build/test steps |
| --- | ---: | ---: | --- |
| Ubuntu Rust | 883,367,373 bytes | 17s | Clippy 55s; tests 5m 24s |
| macOS Rust | 844,650,839 bytes | 23s | Clippy 1m 26s; tests 12m 10s |
| Database adapters | 883,367,373 bytes | 15s | Test prebuild 1m 01s |
| MongoDB | 789,204,956 bytes | 18s | Test prebuild 59s |

None of those four job logs showed DuckDB or SQLite recompilation. All four
Cargo timing HTML reports from the two Rust jobs also contained no
`libduckdb-sys` or `libsqlite3-sys` compilation units. Each Rust job again passed
3328 tests with 12 ignored tests; the database and MongoDB jobs passed 132 and
20 tests. Distribution contracts, real Linux ELF inspection/strip, both Windows
installer hosts, and macOS system-library checks passed.

macOS remains the workflow's longest job. Its test step still compiles and links
workspace test binaries, which the dependency cache excludes; its subsequent
binary dependency check took another 1m 51s. The remaining time does not show
repeated DuckDB native compilation. No manual cache deletion was needed.

## Native Linux container cache validation

An isolated validation branch exercised the committed wrapper and pinned cache
action on native x86_64 and ARM64 Linux runners. It used the release Dockerfile,
approved Ubuntu 22.04 image digest, Rust 1.94.0, identical host/container paths,
and a small executable linked to bundled `libsqlite3-sys` 0.37.0. This verifies
container execution and native dependency artifact reuse without publishing a
release or claiming a full application release build was tested.

| Target | Cold probe build | Warm probe build | Warm cache result |
| --- | ---: | ---: | --- |
| x86_64 Linux | 28s | 2s | Exact hit; native dependency not compiled |
| ARM64 Linux | 44s | 3s | Exact hit; native dependency not compiled |

Sources:

- [Cold container probe, run 39607848245](https://github.com/yelog/lazydb/actions/runs/39607848245).
- [Warm container probe, run 39608185661](https://github.com/yelog/lazydb/actions/runs/39608185661).

Both runs passed on both architectures. The warm cache archives were about
16 MB each, and both executables reported SQLite 3.51.3. Probe build durations
exclude the image preparation and cache restoration steps. The warm jobs still
spent 25–29 seconds preparing their build images.

See [CI builds and dependency caches](../ci.md) for cache ownership, ref access
restrictions, capacity, and the commands used to compare cold and warm runs.
