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
| Rust baseline / current slices | Workspace tests pass with useful regression expectations | Initial baseline failed four portability cases and stopped early. First slice: 711 pass / 3 ignored. Generic, dependency-buffer and cycle-guard slice: 735 pass / 3 ignored across 30 suites | Current Rust suite verified | Root `rtk cargo test --workspace --no-fail-fast`, exit 0; original failures retained below |
| Real Neovim baseline | Starts server, sees capabilities, resolves asserted targets | Original basic passes 9; extended Java fails 5 useful member cases; features fails 3 then stalls. Third fixed release: basic 9, Java 65, features 80, semantic 18, library 13, mixed 36 pass | Current focused editor suite verified | 221 assertions; artifact summaries and mixed compiler snapshots inspected; real-project acceptance continues |
| CEM project model | Minecraft, NeoForge, Create and Kotlin classpaths and sources are visible | Copy resolves 101 materialized classpath entries; offline Kotlin/Java compile succeeds; real Neovim hover and superclass goto reach Create Java source | Partial | Generic member, override, diagnostics and performance cases remain; class goto alone is insufficient |
| Corpus selection | Five additional real, diverse mixed Java/Kotlin repositories with pinned revisions | OkHttp, Moshi, Retrofit, Spring Framework and Gradle pinned and clean; concrete ordinary Java/Kotlin edges identified | Discovery complete | [fork-corpus.md](fork-corpus.md); semantic and performance acceptance still pending |
| Ordinary completion | Correct receiver members, annotation names and valid override proposals in Java/Kotlin | Java `h.as` baseline empty; first fix returns useful members through shared queries; focused Java/Kotlin completion tests pass 143 with 1 ignored | Partial | Verify release editor behavior; annotation/override smart proposals remain pending |
| Inheritance and generics | Correct inherited members and substituted types across both languages | Baseline generic chains fail. Structured Java type arguments and typed Java/Kotlin superclasses resolve release mixed completion/goto both directions. Growing invalid cycles stop without repeated declaration expansion; matching diamonds work and conflicting bindings stay Unknown | Partial; focused repair verified | Direct inherited package collision reproduced: wrong completion and Kotlin goto/diagnostics. Method inference and bounded wildcards also remain |
| Overloads and nullability | Select correct overloads; preserve nullable/platform types; no arbitrary ambiguous target | Not yet measured | Pending | Include invalid and ambiguous cases with independent expected results |
| Kotlin extensions | Visible compatible extensions resolve and complete, including chained results | Not yet measured | Pending | Extend existing semantic/editor probes |
| Unsaved cross-language edits | Both languages immediately observe edited declarations and types without saving | Release mixed fixture: both directions complete and navigate on their first request after both changes are outbound; opened JDK source rename/delete no longer leaves stale facts | Partial; focused repair verified | Normal Neovim cross-buffer debounce observation is separate; actual corpus and repeated edit stress remain pending |
| Rename | Applying workspace edits updates the intended Java/Kotlin declarations and usages only | Baseline omits a Java generic-chain use and renamed snapshot fails compilation. Fixed release and adapted IDEA update all three exact semantic sites, preserve all three complete buffers and compile the renamed snapshot | Mixed fixture verified | Actual corpus, overloads, JVM facades, ambiguous names and extension-provided references remain pending |
| Dependencies and generation | Resolve real Gradle dependencies and generated sources, with correct module visibility | Not yet measured | Pending | Existing gradle-live/library plus real-project cases |
| Extension boundary | Separate consumer reads semantic facts and contributes consistent enabled/disabled results | No extension mechanism validated | Pending | Prove smallest contract after ordinary semantic slice; test config/source invalidation |
| Independent IDEA comparison | Expected member/type/navigation/diagnostic behavior verified in IDEA for acceptance cases | Shared mixed fixture: adapted IntelliJ LSP 38/38 pass and fixed ktlsp 36/36 pass, including strict first responses after change delivery. Earlier ktlsp had seven semantic failures | Shared fixture verified | IDEA has two additional import/readiness checks. Windows URI/version adapter is explicit, vanilla plugin has an applied-rename failure; actual corpus parity remains pending |
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

### Generic and dependency-buffer slice: Rust checkpoint

Root verification on the frozen implementation:

```powershell
rtk proxy powershell -NoProfile -Command '$env:KTLSP_CACHE_DIR="C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/second-slice/rust-cache"; rtk cargo test --workspace --no-fail-fast'
```

Observed: exit 0, **732 passed / 3 ignored across 30 suites**, 3.51 s reported
test execution. The external command log is
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/second-slice/cargo-workspace.log`.
All changed Rust files pass `rustfmt --check --edition 2021`; `git diff --check`
passes. Old dependency symbol and ktcheck durable caches have new format keys,
because normalized type/supertype facts changed.

The checkpoint release at
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/second-fixed/bin/ktlsp.exe`
has SHA-256 `22a43e50bcb80df74f1ded6e3f38322460220994897482fdef78719384b4a554`.
Root inspected successful real-Neovim `second-fixed-mixed` (36/36) and
`second-fixed-library` (13/13) artifacts under
`C:/Users/gummy/AppData/Local/Temp/codex-ktlsp-harness-baseline-20261010`.
Mixed original, unsaved and applied-rename snapshots all compile; all four
post-delivery freshness requests match on attempt 1. This proves the focused
repair, not full IDEA parity or performance targets.

After the generic-cycle guard, root reran the full workspace with an explicit
`KTLSP_CACHE_DIR` under `third-slice/rust-cache`: exit 0, **735 passed / 3 ignored
across 30 suites**, 4.76 s reported test execution. Log:
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/third-slice/cargo-workspace.log`.
All changed Rust files pass the targeted formatting check.

The guarded release is
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/third-fixed/bin/ktlsp.exe`,
SHA-256 `72a900ebe45b4a04daa027d656cd5d7af14c0d9096f5ccaa600a574d344ea7f0`.
Its six real-Neovim scenarios pass **221 assertions**: basic 9, Java 65,
features 80, semantic 18, library 13, mixed 36. Root inspected their summaries
and the mixed result/compile records under the same harness evidence root,
`third-fixed-<scenario>/artifacts`. Invalid self-cycles and two-class cycles
are bounded before expanding a repeated declaration; normal diamonds and
conflicting generic bindings retain their focused regression expectations.

### Next reproduced gap: imported superclass identity

Compiler-valid Java and Kotlin children explicitly inherit `base.Base` while
an unrelated `app.Base` also exists. Javac 21 and Kotlin 1.9.23 accept both
callers; `javap` confirms both children extend `base.Base`. The external
14-check probe passes 7 and fails 7: four completions offer `baseWrong` instead
of `baseOnly`, two Kotlin calls to inherited `accept` navigate to `app.Base`,
and one diagnostics case reports two false argument-shape errors. Java
navigation and diagnostics controls pass.

Evidence: `C:/Users/gummy/AppData/Local/Temp/ktlsp-semantic-gaps-20261010/`
`compile-superclass.ps1`, `superclass-baseline.rs`, `superclass-baseline.exe`,
and `oracle/superclass`. Reproduce with RTK PowerShell `-File
compile-superclass.ps1`, then run `superclass-baseline.exe` (expected exit 1
before repair). Next: preserve typed superclass identities in the remaining
shared completion, call-validation and resolution readers; add unsaved import
switches and correct-target exclusions. This is an external reproduction,
not yet a committed editor acceptance case.

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

The shared `mixed` fixture now verifies ordinary direct receiver completion and
exact cross-language definitions through both engines. Compiler-valid generic
chains `JavaBox<KotlinApi>.value().kotlinGreeting()` and
`JavaBox<JavaApi>.value().javaGreeting()` complete in IntelliJ Server 263.6379.0,
while the immutable first-fixed ktlsp release returns no required completion.
The Java chain also loses its definition/reference/rename use. Evidence roots:
`C:/Users/gummy/AppData/Local/Temp/ktlsp-mixed-development-20261010/baseline-run`
and `oracle-run2`. Repeated requests here wait for indexing and are not timing
samples. The full protocol-neutral references and applied-rename comparison is
still being verified; IDEA may return selector reference ranges and several
minimal edits inside an identifier, so compare semantic sites and complete applied
source rather than assuming identical edit encoding.

The next generic regression baseline has one passing guard and nine failures.
`generic_interop.rs` covers normalized Java type parameters, structural generic
returns, reordered inherited type arguments, qualified same-name types, nullable
substitution and unsaved changes. Independent JDK 21 / Kotlin 1.9.23 extended
compiler controls pass. Its baseline Cargo output is retained at
`C:/Users/gummy/AppData/Local/rtk/tee/1791629734_cargo_test.log`.

### Opened dependency-source identity and late indexing

The existing `library` scenario now opens the actual indexed JDK `Connection.java`
and makes only unsaved changes. Expected behavior is that renaming
`clearWarnings` to `clearWarningsChanged` removes the old completion, deleting the
method removes both candidates, and goto reaches the current declaration.
Closing the dirty source restores its on-disk method; reopening must retain one
current definition. The probe also checks that the source file was never rewritten.

Against the immutable first-fixed release, the complete lifecycle has **11 passing
and 2 failing assertions out of 13**. The failures are stale old completion after
rename and after deletion. Useful closed/new-method completion, exact new-method
goto and close/reopen behavior pass. Evidence:
`C:/Users/gummy/AppData/Local/Temp/codex-ktlsp-harness-baseline-20261010/library-lifecycle-original`.
The earlier `library-lifecycle-baseline` attempt stopped on a test API mistake:
Neovim's detach function returns no boolean. The corrected probe verifies detached
state explicitly and was rerun completely in `library-lifecycle-original`.

A focused pre-fix regression shows the same source indexed as
`C:\Users\gummy\AppData\Local\Temp\ktlsp_dependency_overlay_40216\Helper.java`
while the editor uses
`C:/Users/gummy/AppData/Local/Temp/ktlsp_dependency_overlay_40216/Helper.java`.
The result contains both `clearOriginal` and `clearChanged` instead of only the
current method. Raw failure:
`C:/Users/gummy/AppData/Local/rtk/tee/1791629993_cargo_test.log`.

Dependency `FileSymbols` creation now uses the shared lexical file key, and warm
symbol-cache loading normalizes persisted native path strings. Focused
`rtk cargo test -p ktlsp --lib deps::tests` passes **27 tests** with an explicit
external `KTLSP_CACHE_DIR`. Regressions cover both broad and import-seed parsing,
closed/open source facts, unsaved rename and exact goto, deletion, close/reopen,
and a legacy native-key cache file. No process-global cache variables are changed
inside these tests.

Normalizing identity also exposed a separate late-arrival priority bug. A focused
test delivers closed dependency facts after an open buffer has been edited. The
unchanged merge operation replaces `clearChanged` with `clearOriginal`; raw failure:
`C:/Users/gummy/AppData/Local/rtk/tee/1791630674_cargo_test.log`.
All five LSP dependency/JDK/import merge sites now use
`Workspace::index_dependency_file`, which checks open-buffer ownership in constant
time and skips those closed facts. Callers retain their existing revision batching.
The shared Java/Kotlin regression passes and also verifies unsaved deletion,
acceptance of another closed file and acceptance after the original buffer closes.
Focused command:
`rtk cargo test -p ktlsp --lib workspace::tests::late_dependency_batches_preserve_open_java_and_kotlin_facts`.

The inherited close behavior reindexes disk facts in the volatile tier; this slice
preserves that behavior. No wrong result from that tier transition has been
reproduced. A future duplicate-project/library-qualified-name case can test whether
the closed-library tier needs separate tracking.

The immutable second-fixed release passes the expanded **library 13/13** and
strict **mixed 36/36** scenarios, both exit 0. Its SHA256 is
`22a43e50bcb80df74f1ded6e3f38322460220994897482fdef78719384b4a554`.
Artifacts are under
`C:/Users/gummy/AppData/Local/Temp/codex-ktlsp-harness-baseline-20261010/second-fixed-library`
and `second-fixed-mixed`; the build log is `second-fixed-release-build.log` in
the same evidence directory. Both current document versions are verified outbound
before four first-response freshness checks. Exact references, both applied
renames and compilation of original, unsaved and renamed snapshots pass.

The subsequent immutable third-fixed release includes the reviewed recursive
generic-inheritance guard. Its SHA256 is
`72a900ebe45b4a04daa027d656cd5d7af14c0d9096f5ccaa600a574d344ea7f0`.
The complete editor sweep passes **basic 9, java 65, features 80, semantic 18,
library 13 and mixed 36 assertions**, every scenario exit 0. Evidence is under
`third-fixed-<scenario>/artifacts` in the same external directory. Mixed uses the
extracted shared probe helper and retains all strict freshness, exact-reference,
applied-rename and three compiler checks. These correctness runs were concurrent
and supply no latency acceptance. They do not establish real-project or full IDEA
parity acceptance.

### Shared mixed-source comparison and editor delivery

The complete protocol-neutral comparison uses `dev/ktlsp-harness.sh mixed`,
`dev/mixed_project.lua` and `dev/nvim_mixed.lua`. The Maven fixture contains mutually
referencing Java/Kotlin types and generic `JavaBox<T>` receiver chains. Original,
unsaved-edit and applied-rename snapshots compile independently with Kotlin 2.0.21
and the Java 17 target under JDK 21. Compilation runs on copies outside the indexed
project; the fixture files remain unchanged.

The immutable first-fixed ktlsp release has **27 passing and 7 failing assertions
out of 34** in `baseline-run3`. Generic completion fails in both directions;
Java's generic chain also loses its Kotlin definition, reference and rename site.
Applying the returned rename therefore leaves the Java generic use unchanged and
the renamed snapshot fails compilation. Direct receiver cases and eventual unsaved
completion/navigation pass. Missing generic sites remain failures.

The adapted IntelliJ comparison has **36 passing assertions out of 36** in
`oracle-run4`, with successful Maven import and all three compiler snapshots passing.
References must cover exactly the expected identifier sites with no missing,
duplicate or additional sites. Rename edits must stay inside those identifiers
without overlap, and the complete resulting buffers must equal the expected source.
Selector-shaped reference ranges and minimal substring rename edits are accepted
equally for either engine when they satisfy these semantic checks.

This result requires an explicit Windows URI adapter. The unadapted `oracle-run3`
returns valid edits with equivalent `file:///C:/...` and `file:///c%3A/...` URI
spellings, but the plugin's exact-URI document-version ledger misses the latter.
Neovim rejects edits as stale (`Buffer ... newer than edits.`), only part of the
rename is applied, and the renamed copy fails compilation. This is a reproduced
Neovim/plugin integration limitation and remains separate from the passing IntelliJ
semantic checks. The shared harness normalizes equivalent URIs to open-buffer URIs;
for IntelliJ it then invokes the official plugin's `versions.fix_workspace_edit`
against its existing ledger. Version checks are retained. Raw responses and the
processed `rename-*-apply.json` files are preserved; adapted success is **not a
vanilla IntelliJ-in-Neovim UX pass**.

Both engines' original first Java completion after editing the two buffers fails
before a later response succeeds. Transport logs show that the first request flushes
Java's `didChange`, while Kotlin's notification remains under Neovim's peer-buffer
debounce until after that response. This is an editing-UX gap before delivery,
not evidence that a server ignored an already-delivered Kotlin change. Neovim
0.12.5's `Client:request` calls `changetracking.flush(self, bufnr)`; its existing
`vim.lsp._changetracking.flush(client)` without a buffer sends all pending changes.
The updated scenario records a single normal pre-barrier observation, explicitly
uses that internal all-buffer flush as a **test delivery barrier**, verifies both
current versions on outbound `rpc.notify`, then checks each first completion and
definition response without sleeps or retries. No retrying run supplies timing or
freshness acceptance. The adapted strict IntelliJ run `oracle-run5` passes **38/38**:
both outbound current versions are 3, all four fresh completion/definition requests
match on attempt 1 after the barrier, and all compiler snapshots still pass. Its
normal pre-barrier Java completion fails on its only response and remains recorded.
`mixed-didchange.json` and phase-labelled `mixed-requests.jsonl` distinguish delivery
and first-response observations. The same strict scenario passes **36/36** with
the generic/dependency-source fixes and recursion guard in the immutable third-fixed
release (`ktlsp.exe` SHA-256
`72a900ebe45b4a04daa027d656cd5d7af14c0d9096f5ccaa600a574d344ea7f0`).
All three compiler snapshots pass, both renames preserve the complete expected
buffers, and all four fresh responses match on attempt 1. Artifact:
`C:/Users/gummy/AppData/Local/Temp/codex-ktlsp-harness-baseline-20261010/third-fixed-mixed`.
Extracting the common engine/position helpers for the corpus preserves IntelliJ's
**38/38** result in `oracle-run6`. This establishes the ordinary mixed fixture;
real-project breadth, unsaved editing latency and full IDEA parity remain open.

All comparison artifacts above are under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-mixed-development-20261010`.
