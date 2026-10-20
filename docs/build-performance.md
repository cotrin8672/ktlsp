# Development build measurements

The workspace uses `debug = "line-tables-only"` for dev builds and disables debug
information for non-workspace dependencies. Cargo's test profile inherits dev.
Workspace backtraces retain filenames and line numbers; variable/type inspection
requires a rebuild with `cargo --config profile.dev.debug=2 build -p ktlsp --bin ktlsp`.
Dependency debugging additionally requires overriding `profile.dev.package."*".debug`.
These settings follow the [Cargo build-performance guidance](https://doc.rust-lang.org/cargo/guide/build-performance.html).

Optimization, incremental compilation, codegen units, assertions and overflow checks
keep their existing defaults. Release remains Cargo's default optimized profile
(`opt-level = 3`, non-incremental, 16 codegen units, local ThinLTO where applicable),
as described in [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html).
Use release binaries for server runtime comparisons; these measurements concern
build latency and establish no server runtime improvement or IDEA parity.

## Method

Measured on 2026-10-21 JST at revision `1b34ec776183a224cf3cd2811595e595f67913ba`
with the pre-existing working changes retained: Windows x86_64 MSVC, Ryzen 7 5700X
(8 cores/16 logical CPUs), rustc 1.98.1 (`48a229cea`), Cargo 1.98.1 (`797e8a9bc`).
No Cargo configuration or profile environment overrides were present. The existing
target directory was reused without cleaning (debug 76.61 GiB, release 1.55 GiB
before the run; approximately 456 GiB free). No competing Cargo job was observed.

Each configuration was prepared once, followed by three unchanged invocations and
three alternating equivalent implementation edits. Timings include Cargo, RTK
and process startup, compilation/linking and, for tests, execution of all five
selected tests. They exclude the edit operation and initial cache preparation.
Both profiles use identical commands, edits, package selection and target cache.

- Core: `rtk proxy cargo test -p ktcore --lib coords::tests --locked`;
  `crates/ktcore/src/coords.rs`, `xn.cmp(&yn)` versus `u64::cmp(&xn, &yn)`.
- Server: `rtk proxy cargo build -p ktlsp --bin ktlsp --locked`;
  `crates/ktlsp/src/trace.rs`, `now_us()`'s `.unwrap_or(0)` versus `.unwrap_or_default()`.

Both edits change Rust implementation tokens and preserve behavior. They represent
small edits, including reuse of previously compiled variants; they do not model
arbitrary new types, large refactors, cold builds or every developer workload.
The core sequence was baseline then candidate; the server sequence was candidate
then baseline to reuse the just-prepared server cache. Files were restored byte
for byte after each sequence, with SHA-256 restoration records.

## Evidence

Three samples per cell, seconds, p50 (median) with observed minimum–maximum.
For n=3, nearest-rank p95 is the maximum shown in each cell; it does not
estimate a population tail-latency percentile.

| Workflow | Default full debug | Reduced dev debug |
| --- | --- | --- |
| Core unchanged test | 1.473 (1.357–1.917) | 1.312 (1.258–1.634) |
| Core implementation edit → test | 5.819 (5.817–5.994) | 4.646 (4.376–4.755) |
| Server unchanged build | 1.781 (1.599–1.791) | 1.965 (1.845–2.141) |
| Server implementation edit → build | 16.623 (15.815–26.230) | 16.446 (15.319–17.444) |

The core edit/test median decreased by about 20%; the server edit/build median
changed by about 1%, which is insufficient evidence of a meaningful server build
speedup. Unchanged invocations are dominated by startup and cache checks; no
speedup is claimed for them. Three samples are a small local observation, not a
tail-latency guarantee. The baseline server run included a 26.230 s outlier.

With the same final edited server source, full-debug versus reduced-debug
artifacts were 40,585,728 versus 36,868,096 bytes for the EXE and 265,662,464 versus
91,754,496 bytes for the PDB (about 65% less PDB storage). This does not measure
total target-directory savings; old fingerprints and incremental caches remain.

### Windows development linker command

The installed MSVC Rust toolchain includes `rust-lld.exe`. A second comparison
kept the reduced-debug profile and used
`rtk proxy cargo rustc -p ktlsp --bin ktlsp --locked --timings -- -C linker=LINKER`
with `link.exe` then `rust-lld.exe`. This changes only the final server binary's
linker; [Cargo rustc](https://doc.rust-lang.org/cargo/commands/cargo-rustc.html)
passes the extra flags only to that selected target. Each phase again had one
preparation invocation, three unchanged invocations and three identical alternating
`trace.rs` implementation edits, including rebuilding the ktlsp library.

| Workflow | MSVC LINK | Bundled LLD |
| --- | --- | --- |
| Unchanged server | 2.071 (1.890–2.312) | 1.801 (1.752–1.916) |
| Server implementation edit → build | 13.784 (13.658–14.498) | 12.810 (11.985–13.399) |

The edit/build median decreased by about 7.1% in this separate three-sample
comparison. It is not an additional percentage to combine with the first table.
The third sample's Cargo timings show library compilation of 5.97 s with MSVC
versus 6.12 s with LLD, and final binary compilation/linking of 5.68 s versus
4.68 s. Cargo does not isolate pure linker time here. The library frontend alone
took 4.32/4.54 s, so linker selection does not remove the remaining compilation
cost. `server-msvc-timing.html` and `server-lld-timing.html` retain these reports.

Use `cargo rustc -p ktlsp --bin ktlsp -- -C linker=rust-lld.exe` for this Windows
development workflow, and `dev/ktlsp-harness.sh basic --no-build` to validate that
binary without rebuilding it with the default linker. There is no global linker
setting, new dependency or machine-specific repository path. An ordinary
`cargo build` still uses the default linker; switching commands may rebuild the
final binary. Release builds retain their original settings and linker.

Local run directory:
`C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda/build-speed120/`.
The JSON files record every command, elapsed time, exit code and log path.
`baseline-source.json`, `candidate-source.json`, and the corresponding server
records retain source restoration hashes. `measure.ps1` records the exact local
procedure; it is an external measurement aid, not a new repository test harness.

No cold build was measured. Profile transitions can rebuild dependencies and
cost more than normal editing. The candidate's initial core preparation took
34.415 s; preparing the server and bench from the new profile took 112.008 s.
The baseline core preparation took 30.036 s with pre-existing caches, so these
preparation timings are not a controlled cold-build comparison.

The baseline server preparation took 85.462 s. The separate linker-comparison
preparations took 6.713 s for MSVC and 13.677 s for LLD; the latter also rebuilt
the restored original library source. These are excluded from edit/build medians.

## Verification and remaining scope

| Case | Expected | Observed/status | Evidence and next action |
| --- | --- | --- | --- |
| Core test behavior | All five selected tests pass in both configurations and source variants | Pass, every invocation | `baseline*.json`, `candidate*.json` and logs; expand measurements only for a new workload |
| Workspace type checking | All workspace targets check with adopted dev settings | Pass | `workspace-check.log`, 72.618 s; no source semantics changed |
| Workspace backtrace | Workspace-style panic frame retains filename and line | Pass | `panic-probe.log` contains `panic_probe.rs:2`; tiny standalone rustc probe with the same line-table setting, not full dependency debugger validation |
| Native editor startup/navigation | Server initializes and resolves local/cross-file definitions | Pass with MSVC and LLD | `harness-basic/artifacts/summary.txt` and `harness-lld-basic/artifacts/summary.txt`; both status 0 |
| Source restoration | Original source bytes after measurements; final server built from restored source | Pass | `*-source.json`, `final-build.log`, `final-lld-build.log`; source edits are not part of the patch |
| Optimized runtime | Preserve original release optimization/linker choices | Configuration preserved; runtime unmeasured | No release override added; use a separate release runtime baseline before claiming runtime gains |

The harness's first attempt through a different Git installation failed before
launch (`Top-level not found`); the successful runs used the existing Scoop Git
Bash. This environment failure is excluded from build measurements. The basic
probe stops its server after assertions and reports signal 15; both harness
summaries still report status 0. No full workspace test rerun, cold build, release
runtime comparison, or alternative operating-system measurement was performed for
this build-only change.

The native MSVC linker was observed. No additional linker or nightly compiler
requirement was introduced. Microsoft's obsolete `/DEBUG:FASTLINK` was excluded:
it is [removed in Visual Studio 2026](https://learn.microsoft.com/en-us/cpp/build/reference/debug-generate-debug-info?view=msvc-170).
