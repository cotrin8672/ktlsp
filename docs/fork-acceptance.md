# Fork acceptance and evidence

Updated: 2026-10-10 (Asia/Tokyo). Goal remains **in progress**.

## Scope

Deliver a fast, shared Java/Kotlin editing experience in Neovim for Minecraft mod
development. Acceptance requires useful and correct completion, inference,
diagnostics, navigation, references and rename across both languages and unsaved
buffers, using the same Gradle dependencies and generated sources.

Provide a public analysis extension boundary for a separate Mixin support project.
The extension must read shared semantic results and contribute facts used consistently
by existing features. A small representative Mixin test may prove that boundary;
shipping a Mixin implementation is outside this project's scope. Source and extension
configuration changes must invalidate contributed facts and cached results.

The real-project corpus is CreateEnchantableMachinery (CEM), excluding Mixin code
from acceptance probes, plus five additional distinct Java/Kotlin projects. Small
generated fixtures supplement that corpus; they do not replace it.

## Initial environment

- Starting revision: `f2c4c96799f68e11014021b7a77a0ef950770da7`, branch `main`.
- Checkout: `C:/Users/gummy/ghq/github.com/cotrin8672/ktlsp`.
- Existing user change: `AGENTS.md` (preserve).
- Host: Windows 11 Home 10.0.26200 x64; Ryzen 7 5700X (8 cores / 16 threads),
  68,640,747,520 bytes physical RAM; Rust/Cargo 1.98.1; Neovim 0.12.5; JDK 21.
- Git Bash: `C:/Users/gummy/scoop/apps/git/current/bin/bash.exe`.
- `bash` on the default Windows PATH launches WSL instead of Git Bash.
- All scripted server runs use an explicit disposable `KTLSP_CACHE_DIR`.
- Baseline Rust command: `rtk cargo test --workspace` (failed in ktcheck tests).
- Baseline editor command: existing `dev/ktlsp-harness.sh basic`, then `java`,
  `features` and `semantic`, using a release binary and external artifact directories.

Commands in this record are run through RTK as required by the local instructions.
Record exact artifact paths and revisions when each check completes. An attempted
command, an advertised capability or an empty successful RPC is not a semantic pass.

## Acceptance matrix

| Case | Expected result | Observed result | Status | Evidence / next action |
| --- | --- | --- | --- | --- |
| Rust baseline / first slice | Workspace tests pass with useful regression expectations | Initial baseline failed four portability cases and stopped early. After first fixes and integration fixture repairs: 711 pass / 3 ignored across 29 suites | First slice verified | Root `rtk cargo test --workspace --no-fail-fast`, exit 0; original failures retained below |
| Real Neovim baseline | Starts server, sees capabilities, resolves asserted targets | Original basic passes 9; extended Java fails 5 useful member cases; features fails 3 then stalls. Fixed release: basic 9, Java 65, features 80, semantic 18 pass | First slice verified | Artifact summaries and assertions inspected; library and real-project acceptance continue |
| CEM project model | Minecraft, NeoForge, Create and Kotlin classpaths and sources are visible | Copy resolves 101 materialized classpath entries; offline Kotlin/Java compile succeeds; real Neovim hover and superclass goto reach Create Java source | Partial | Generic member, override, diagnostics and performance cases remain; class goto alone is insufficient |
| Corpus selection | Five additional real, diverse mixed Java/Kotlin repositories with pinned revisions | OkHttp, Moshi, Retrofit, Spring Framework and Gradle pinned and clean; concrete ordinary Java/Kotlin edges identified | Discovery complete | [fork-corpus.md](fork-corpus.md); semantic and performance acceptance still pending |
| Ordinary completion | Correct receiver members, annotation names and valid override proposals in Java/Kotlin | Java `h.as` baseline empty; first fix returns useful members through shared queries; focused Java/Kotlin completion tests pass 143 with 1 ignored | Partial | Verify release editor behavior; annotation/override smart proposals remain pending |
| Inheritance and generics | Correct inherited members and substituted types across both languages | Ordinary Java inheritance covered; compiler-valid Java generic returns and Kotlin generic supertype substitution return no useful completion | Failed | Exact independent compiler controls and probes below; next shared semantic slice |
| Overloads and nullability | Select correct overloads; preserve nullable/platform types; no arbitrary ambiguous target | Not yet measured | Pending | Include invalid and ambiguous cases with independent expected results |
| Kotlin extensions | Visible compatible extensions resolve and complete, including chained results | Not yet measured | Pending | Extend existing semantic/editor probes |
| Unsaved cross-language edits | Both languages immediately observe edited declarations and types without saving | Focused tests pass Java receiver-type and Kotlin member edits observed by Java completion | Partial | Real Neovim and IDEA shared fixture; both-direction completion/definitions/references still required |
| Rename | Applying workspace edits updates the intended Java/Kotlin declarations and usages only | Not yet measured | Pending | Apply edits and inspect resulting buffers; compile disposable sources |
| Dependencies and generation | Resolve real Gradle dependencies and generated sources, with correct module visibility | Not yet measured | Pending | Existing gradle-live/library plus real-project cases |
| Extension boundary | Separate consumer reads semantic facts and contributes consistent enabled/disabled results | No extension mechanism validated | Pending | Prove smallest contract after ordinary semantic slice; test config/source invalidation |
| Independent IDEA comparison | Expected member/type/navigation/diagnostic behavior verified in IDEA for acceptance cases | Official IntelliJ LSP plugin runs through real Neovim: Maven import, completion, navigation, refs, diagnostics and applied quickfix pass upstream integration smoke | Oracle setup complete | Shared ktlsp/IDEA mixed-source comparison still pending; no parity claim |
| Correctness plus latency | Meaningful responses meet recorded budgets on all six projects | Not yet measured | Pending | Release build; sample counts, p50/p95, cold/warm distinction and RSS |

## Measurement policy and initial UX targets

Before claiming speed, collect a native-host baseline and retain each sample alongside
its semantic assertion. The following initial targets express the desired editor UX;
they are **unverified targets**, not measured results or a claim of IDEA parity:

- Warm completion protocol latency: p95 <= 100 ms, at least 30 valid samples per
  project and request class.
- Input-to-completion-display latency in Neovim: p95 <= 200 ms, measured separately
  from RPC latency with an actual completion UI probe.
- Warm definition/hover: p95 <= 100 ms; references/rename: p95 <= 500 ms.
- Unsaved-edit result freshness: correct result on the first request after delivery
  of the document change; report debounce and end-to-end time separately.
- Cold first useful project result: <= 10 s; full dependency/JDK readiness: <= 120 s.
  Both require at least three cold process runs with isolated caches.
- Warm restart readiness: <= 5 s, at least five process restarts.
- Report steady-state working set/RSS and peak memory separately; set a concrete
  memory budget after baseline project sizes and host capacity are recorded.

Record baseline-derived budget decisions before accepting performance changes.
Report failures and missing semantic results separately; fast empty responses cannot
lower the accepted latency distribution. Sampling/profiling runs are separate from
timing runs. Initialization timing never substitutes for editor completion timing.

## Work log

### Initial investigation

Baseline, corpus discovery and semantic gap investigation started in parallel using
`gpt-6.1-sol` agents at `max`. No semantic behavior has changed. Initial checks confirm
native Rust and Neovim availability and the existing user change noted above.

Baseline evidence root:
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/baseline`.
`cargo-workspace.log` and `cargo-core-lsp.log` retain the original Cargo outputs.
`bin/ktlsp.exe` and `bin/bench.exe` preserve the original release build for comparison.

The four baseline Rust failures are:

1. `ktcheck::tests::durable_cache_inputs_track_gradle_metadata_only`: slash-only
   string comparison against Windows paths.
2. `ktcheck::tests::prune_durable_snapshots_keeps_current_and_one_previous_snapshot`:
   Rust test name containing `::` used as a Windows directory component.
3. `deps::tests::gradle_root_prefers_gradlew_over_build_file`: Unix-only wrapper
   fixture does not model Windows `gradlew.bat`.
4. `deps::tests::concurrent_android_import_resolution_shares_lazy_extraction_safely`:
   slash-only assertion; also investigate cold/warm source-key consistency.

Real Neovim basic command (through Git Bash):
`bash dev/ktlsp-harness.sh basic --release --no-build --bin C:/Users/gummy/ghq/github.com/cotrin8672/ktlsp/target/release/ktlsp.exe --run-dir /tmp/codex-ktlsp-harness-baseline-20261010/original-basic-exe`.
All 9 assertions passed. Artifacts:
`C:/Users/gummy/AppData/Local/Temp/codex-ktlsp-harness-baseline-20261010/original-basic-exe/artifacts/summary.txt`.
Windows Neovim uses `xdg-state/nvim-data/lsp.log`; the original harness reports the
Unix path instead. Forced client shutdown emits a signal message; goto assertions
are verified, clean shutdown performance is not.

Java member gap reproduction:
`C:/Users/gummy/AppData/Local/Temp/ktlsp-semantic-gaps-20261010/probe.rs`, compiled
against the baseline Rust library. Java `Helper h; h.as` gives `None`, equivalent
Kotlin gives `Some([assist])`. The Java adapter intentionally rejects after-dot
completion; the existing `omits_after_dot` test encodes that missing coverage.
Compiler-valid controls under `oracle/java/app/*.java` compile with JDK 21 `javac`.
Further reproduced gaps retained for later slices: Java generic type facts, generic
supertype substitution, and completion of generic Kotlin extension receivers.

### First semantic and Windows source-discovery slice

Java after-dot completion now uses the same member candidate/query and shaping
flow as Kotlin. Java syntax recovery supplies a receiver and recovered current-file
facts; it does not mutate canonical definition offsets or clone the full workspace
index. Java sees Kotlin class members and current unsaved declarations, while Kotlin
extension functions are excluded from Java instance-member completion.

Focused validation: `rtk cargo test -p ktlsp --test java_completion --test completion`
passes 143 tests with one existing ignored test. Independent JDK 21 / Kotlin 1.9.23
compiler controls are retained under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-semantic-gaps-20261010/oracle`.

Windows Git source discovery compared non-verbatim Git paths against verbatim
canonical roots and silently found zero files. Relative-root containment and shared
lexical file keys now preserve project files and unsaved editor buffers. Focused
tests cover staged/untracked/ignored files, nested/canonical roots, generated roots,
native/URI buffer identity and nested Gradle cache inputs. Four initial portability
failures are repaired. Broad `rtk cargo test --workspace --no-fail-fast` then exposes
six additional integration failures (E2E fixture identity, Windows path assertions,
Unix-only formatter fixture). These are repaired with native file URIs, path-component
assertions and byte-preserving platform formatter fixtures; exact definition spans
and useful results remain asserted. Root rerun of the full command passes **711 tests,
3 ignored, 29 suites** (reported test execution 6.21 seconds; compile/wait time is
separate). Initial failing output is retained as
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/first-slice/cargo-workspace-initial.log`.

`rtk cargo fmt --all -- --check` reports existing formatting differences in unchanged
`ktcore` indexed diagnostics/resolution/types and ktlsp classpath/JAR code. Changed
files are formatted separately; unrelated files are not silently rewritten.

Baseline Neovim artifacts are under
`C:/Users/gummy/AppData/Local/Temp/codex-ktlsp-harness-baseline-20261010`:
`original-basic-exe` (9 pass), `original-java-exe` (56 pass),
`extended-java-baseline` (60 pass / 5 fail). `original-features-exe` is a partial,
supervisor-terminated run: 44 assertions pass, 3 fail, then a later request stalls.
Its trace/output remain evidence; it is not a complete passing run.

Compiler-valid coverage gaps preserved for the next slice include
`Box<Item>.get().use`, Java factory generic-return chains, inherited
`KotlinBase<Item>.get().use`, generic Kotlin extension receivers, and compact valid
single-line Kotlin declarations that the parser rejects. Empty successful RPCs
for these cases are failures, regardless of latency.

The first fixed release (`ktlsp.exe` SHA-256
`55076a00b0fbe2bbe1f719a74751a5f2bc026934f4ef66d2c586cf056d8f2a47`)
passes existing real Neovim scenarios `basic` (9 assertions), expanded `java` (65),
`features` (80), `semantic` (18) and `library` (2 useful stdlib/JDK definition assertions).
The library script now honors the harness's `KTLSP_BIN`; its artifact is
`fixed-library-env`. Other artifacts use the same baseline directory with
`fixed-` prefixes. The previously failing Java member cases and imported Kotlin
navigation now pass; the feature scenario completes. These small fixtures establish
the first slice, not real-project or IDEA parity. The scripts force-stop clients,
so their shutdown messages do not establish clean shutdown behavior or memory.

CEM's current-source disposable copy passes `classes --offline --console=plain`
with Kotlin 2.4.0 and JDK 21 in 47 seconds, including the original Java sources.
Compiler warnings are retained in `cem-classes-offline.log`; this is neither a
Mixin runtime check nor an instruction to fix the user's project. The ordinary
Kotlin `EnchantableDrillBlock` superclass probe passes all ten assertions through
Neovim, including hover before goto and navigation to Create's `DrillBlock.java`.
Artifact: `cem-drill-superclass` in the baseline harness directory. Exact command:
`dev/ktlsp-harness.sh project --release --no-build --root <cem-working-copy> --file <copy>/src/main/kotlin/io/github/cotrin8672/cem/content/block/drill/EnchantableDrillBlock.kt --definition-token DrillBlock --definition-occurrence 4 --definition-target DrillBlock.java --run-dir /tmp/codex-ktlsp-harness-baseline-20261010/cem-drill-superclass`.
An earlier run at textual occurrence 2 selected the `DrillBlock` substring inside
the `DrillBlockEntity` import and correctly reached that entity. Its target assertion
failed because the probe cursor was configured incorrectly; it is not a ktlsp bug.

### Neovim IntelliJ oracle

First implementation checkpoint: `25a6a0980b4726036ae7c6a6d77da6a3dcfa7179`,
published on `origin/codex/java-kotlin-analysis`. The user's pre-existing `AGENTS.md`
modification is excluded. Later slices continue on that branch.

At the user's request, comparison uses the official
[JetBrains intellij-lsp.nvim](https://github.com/JetBrains/intellij-lsp.nvim) plugin,
pinned to `9cfad18005470e4187090013e08f92ec9e437219`, and IntelliJ Server 2026.3 EAP
build `263.6379.0`. The cached server binary is reused read-only. Plugin, Maven 3.10.0,
editor configuration, server home, caches and artifacts are isolated under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-intellij-oracle-20261010`; the user's Neovim
configuration is unchanged. Computer-use verification was stopped at the user's
direction and provides no semantic evidence.

Upstream `test/java_e2e.lua` passes through Neovim 0.12.5 after a verified temporary
Maven installation. The first attempt failed Maven import and is recorded as a
failure; the subsequent run confirms import status `SUCCESS`, useful Java/JDK
completion and definitions, references, diagnostics and an applied quickfix.
Evidence: `artifacts/plugin-java-e2e-maven.log` under the oracle directory. This proves
the comparison setup, not ktlsp/IDEA equivalence. Next compare identical mixed
sources and unsaved edits using the repository's existing editor harness.
