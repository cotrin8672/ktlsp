# Fork acceptance and evidence

Updated: 2026-10-11 (Asia/Tokyo). Goal remains **in progress**.

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
| Rust baseline / current slices | Workspace tests pass with useful regression expectations | Initial baseline failed four portability cases and stopped early. Eighth immutable slice: 875 pass / 3 ignored across 35 suites | Eighth Rust suite verified | Root `rtk proxy cargo test --workspace`, native exit 0 with an isolated cache; `eighth-fixed/cargo-workspace-isolated.{out,verified.json}`. All 93 Rust/build inputs match before/after tests and release compilation |
| Real Neovim baseline | Starts server, sees capabilities, resolves asserted targets | Fifth release passes basic 9, Java 99, features 80, semantic 18, mixed 36 and library 13, plus two native UI samples. Seventh passes Java 165/165, including all original 99 expectations and 66 member/unsaved-edit/rename assertions | Java current slice verified; broader rechecks continue | `ktlsp-semantic-gaps-20261010/member-editor-seventh/artifacts/`. Sixth's eight initial Java failures are repaired by immediate initial scanning; no harness sleep was added. Its legacy forced editor shutdown is not graceful-shutdown proof |
| CEM project model | Minecraft, NeoForge, Create and Kotlin classpaths and sources are visible | Eighth release reads 5,865 NeoForge source files with 103,029 symbols; generic-holder completion and exact navigation pass. All six incomplete requests pass; superclass completion/navigation and three missing source archives still fail | Partial; real editing repair verified | `eighth-cem-observations.json` and guarded golden/incomplete runs under the corpus evidence root. Full CEM diagnostics, overrides and performance remain |
| Corpus selection | Five additional real, diverse mixed Java/Kotlin repositories with pinned revisions | OkHttp, Moshi, Retrofit, Spring Framework and Gradle pinned and clean; concrete ordinary Java/Kotlin edges identified | Discovery complete | [fork-corpus.md](fork-corpus.md); semantic and performance acceptance still pending |
| Ordinary completion | Correct receiver members, annotation names and valid override proposals in Java/Kotlin | Java `h.as` baseline empty; first fix returns useful members through shared queries; focused Java/Kotlin completion tests pass 143 with 1 ignored | Partial | Verify release editor behavior; annotation/override smart proposals remain pending |
| Incomplete editing | Useful context-aware completion in unfinished Java/Kotlin, including syntax errors, partial names, missing delimiters and unresolved argument types | Sixth six-root controls pass 15/30 cases and 18/36 qualified requests; IDEA passes 24/30 and 29/36. Eighth separately repairs CEM to 5/5 cases and 6/6 first-response requests | Partial; real CEM repair verified | [fork-corpus.md](fork-corpus.md) records exact per-root results, source hashes and remaining missing inputs. The other five roots require current rechecks; a paired useful known receiver is required for negative cases |
| Inheritance and generics | Correct inherited members and substituted types across both languages | Structured Java type arguments and typed superclasses resolve mixed completion/goto both directions. Fourth release repairs imported superclass collisions across completion, goto and diagnostics; unsaved import switches are fresh on the first response. Growing cycles are bounded and incomplete hierarchy facts remain Unknown | Partial; focused repairs verified | Java 99 includes 34 new editor assertions. Method inference, bounded wildcards, declaration shadowing and real-project coverage remain |
| Overloads and nullability | Select correct overloads; preserve nullable/platform types; no arbitrary ambiguous target | Not yet measured | Pending | Include invalid and ambiguous cases with independent expected results |
| Kotlin extensions | Visible compatible extensions resolve and complete, including chained results | Not yet measured | Pending | Extend existing semantic/editor probes |
| Unsaved cross-language edits | Both languages immediately observe edited declarations and types without saving | Release mixed fixture: both directions complete and navigate on their first request after both changes are outbound; opened JDK source rename/delete no longer leaves stale facts | Partial; focused repair verified | Normal Neovim cross-buffer debounce observation is separate; actual corpus and repeated edit stress remain pending |
| Rename | Applying workspace edits updates the intended Java/Kotlin declarations and usages only | Baseline omits a Java generic-chain use and renamed snapshot fails compilation. Fixed release and adapted IDEA update all three exact semantic sites, preserve all three complete buffers and compile the renamed snapshot | Mixed fixture verified | Actual corpus, overloads, JVM facades, ambiguous names and extension-provided references remain pending |
| Dependencies and generation | Resolve real Gradle dependencies and generated sources, with correct module visibility | Sixth release repairs the Gradle 7 regression: Moshi passes 3/3 golden and 6/6 incomplete requests. Gradle retains 3/3 exact golden targets. All six model-v3 snapshots have complete source sets and zero unresolved entries; five roots still lack dependency sources | Partial; model repair verified | [fork-corpus.md](fork-corpus.md) records all twelve immutable runs, unchanged inputs, exact remaining semantic gaps and independent missing-source failures |
| Live Gradle changes | An already-running client reflects dependency additions, version changes, removals and new build outputs | Seventh native automatic watching passes 74/76: four actual builds, dependency/generated-source additions, updates, removals, exact targets, preserved dirty buffers and clean shutdown. Three native notification batches and zero explicit notifications. Two stale-member diagnostics remain failing | Partial; automatic refresh verified, diagnostics failing | `ktlsp-gradle-model-20261010/seventh-refresh-automatic/artifacts/`. Unchanged 76 expectations and hashes. Sixth manual 70/72 and automatic 29/76 remain preserved; external Windows Neovim watch roots still need coverage |
| Compiler plugins and processors | Analyze generated and transformed APIs used from both languages, including unsaved source changes | Sixth passes 144/144 generated-source checks; IDEA passes 140/144, retaining four exact-source failures. Seventh same-client KAPT/KSP refresh passes 97/114; transformed-API completion still fails 12/12 | Partial; live editing and transformed APIs failing | Portable processor fixtures and raw evidence below. Actual v1/v2/restored builds and disabled compiler rejections independently establish expected APIs. Parser, stale-member and removed-import diagnostics remain gaps |
| Extension boundary | Separate consumer reads semantic facts and contributes consistent enabled/disabled results | Public semantic-store contract passes 22 API tests; source-fact producer passes eight. Independent Mixin oracle has 42 cases. No editor host/consumer integration is validated | Pending integration | Prove enabled/disabled completion, diagnostics, navigation and applied rename through a separate consumer; API tests alone do not satisfy the boundary |
| Independent IDEA comparison | Expected member/type/navigation/diagnostic behavior verified in IDEA for acceptance cases | Shared mixed fixture: adapted IntelliJ LSP 38/38 pass and fixed ktlsp 36/36 pass, including strict first responses after change delivery. Earlier ktlsp had seven semantic failures | Shared fixture verified | IDEA has two additional import/readiness checks. Windows URI/version adapter is explicit, vanilla plugin has an applied-rename failure; actual corpus parity remains pending |
| Correctness plus latency | Meaningful responses meet recorded budgets on all six projects | Not yet measured | Pending | Release build; sample counts, p50/p95, cold/warm distinction and RSS |

## Actual KAPT/KSP compile and runtime controls

Root independently built real Java JSR-269/KAPT and Kotlin KSP processors using
Gradle 7.4.2, Kotlin/KAPT 1.8.21, KSP 1.8.21-1.0.11 and JDK 17. They generate
`KaptJava.java`, `KspJava.java` and `KspKotlin.kt`; the consumer calls all three
from Java and Kotlin. No generated API was authored in an input stub. Evidence
is under `C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/processor-oracle/evidence/`.
The first offline setup attempt failed on unavailable plugin-marker metadata;
that log is retained separately. The subsequent actual online build succeeds,
including KSP, KAPT, Kotlin and Java compilation.

Changing only the real generation option from v1 to v2 replaces `versionOne`
with `versionTwo` in all three emitted APIs. Both builds exit 0 and each runtime
control passes 12 assertions: six calls through the Java/Kotlin consumers, three
present versioned methods and three absent obsolete methods. Sources and hashes
are retained in `generated-api-v{1,2}` and their corresponding source JSON files.
`processor-api-v{1,2}-build.{log,json}` and `runtime-api-v{1,2}.json` retain the
compiler/runtime results. This is actual processor execution, not a model-only
simulation.

After `:app:clean :app:classes -PprocessingDisabled`, the expected Kotlin build
failure names all three missing types. Gradle does not reach Java compilation,
so a separate actual javac invocation checks the Java consumer and also rejects
all three missing types. All three generated source files are absent.
`processor-off-controls.json` records both exits and the six rejection/source
controls. Re-enabling v1 processing rebuilds successfully, passes all 12 runtime
assertions and reproduces all three original generated source hashes exactly;
see `processor-restored-v1-build.{log,json}`, `runtime-restored-v1.json` and
`generated-restored-v1-checks.json`.

The earlier post-build model owns both KSP roots but omits KAPT's generated Java
root (`processor-model-root-checks.json`, 2 pass / 1 fail). Reading public
`JavaCompile.source` inputs repairs that omission. The actual updated Gradle 7
dump owns all three generated files, while a modern Gradle 8.10.2 strict
configuration-cache cold/warm control retains custom Java/Kotlin and processor
roots, excludes a resource-file source decoy and avoids broadening to the whole
project. Those 16 model checks pass in
`C:/Users/gummy/AppData/Local/Temp/ktlsp-gradle-model-20261010/model-input-proof.json`,
with init-script SHA-256
`df30f6b97bef288a0549e849189c103c4a556929555e68dd153c1dc002323130`.
Root ownership derived from a filtered or exact-file JavaCompile FileTree can
still be less precise than the compiler's file membership; that limitation
remains explicit. Model version 3 invalidates older persisted model entries.

The portable producer is `dev/processor_project.lua`, with the existing corpus
route and `dev/corpus/processor-generated.json`. Root generated a fresh external
`processor-portable-1` project, compiled all 11 real tasks successfully offline
and passed its source-file Java runtime control (12/12). All 102 pinned source,
generated-target and edit-anchor preflight assertions pass in
`processor-portable-evidence-1/manifest-preflight.json` under the external goal
directory. Its 24 editor cases require 30 useful/paired completion responses
and six exact generated-source definitions.

The immutable sixth release, SHA-256
`fa0148d65badc1cb8e95683a5d7bb76a742795a75532afed07e6567be2bfdf90`,
passes all 144 editor assertions against that fresh portable project. All 30
completion requests and six exact generated-source definitions match on attempt
1. All 36 edit/restore delivery controls pass, both authored consumer files are
unchanged, the three shared harness hashes are stable, and the client stops.
The index reports 7 project files and 14,308 library files, with zero failed
dependencies or missing sources. Raw results and all 36 requests are under
`processor-editor-ktlsp-sixth-1/artifacts/`; `wrapper-result.json` records exit 0.
This verifies post-build generated-source editing. Normal same-client processor
configuration/build refresh was still unmeasured at that post-build checkpoint.

The subsequent `KTLSP_PROCESSOR_RELOAD=1` probe keeps one real client through
v1, v2, disabled processing and restored v1. The three enabled actual builds
exit 0 and each passes 12 runtime checks. Disabled compilation exits 1 with all
three generated types unresolved; a separate javac control rejects all three,
and their emitted source files are absent. All three native watched-file
notification batches arrive, the current completion API and 24 exact source
targets/proved removals pass, and restored source hashes equal the original
three hashes. Authored disk files remain unchanged and the client exits cleanly.

The whole run retains exit 1 and 97/114 passing assertions. Seventeen assertions
fail: eight unsaved `Source.editorUnsaved` completions (already empty in phase 0,
so not established as a refresh regression), six stale-member diagnostics and
three Kotlin removed-type diagnostics. Refresh readiness uses 102 separate
polling requests; its 60 acceptance requests must not be advertised as first
responses after a build or as performance samples. Evidence is
`processor-refresh-seventh-1/artifacts/processor-reload.json` and its wrapper
record under the external goal directory. All four script hashes and the binary
hash are unchanged. This is real processor execution and native watch delivery, but
the remaining failures prevent complete processor editing acceptance.

The official IntelliJ LSP plugin against that same portable project passes all
30 completion requests on their first responses after successful Gradle import,
including every incomplete and paired unknown case. Exact generated-source
navigation passes 2/6: Kotlin to KSP Kotlin/Java sources. Kotlin to KAPT Java,
and Java to all three APIs, instead return their compiled `.class` files. The
original source-definition expectations are unchanged, so the run retains
140 passing assertions and four failures, wrapper exit 1. Both queried source
files are unchanged, all three harness scripts have matching before/after
hashes, and the client reports stopped. Evidence is `processor-editor-intellij-1/`:
`observations.json`, `wrapper-result.json`, and the complete corpus results and
36 raw requests under `artifacts/`. This records the plugin's source-navigation
limit rather than accepting a different target as the exact source. The wrapper
finished in 39.8 seconds; an inherited child output pipe remains open in the
command runner, so that observation is not a process-lifetime or latency claim.

## Compiler-plugin expectations and first independent control

Compiler-plugin coverage must include both emitted source and APIs transformed
without a generated source file. KSP emits additional source rather than changing
the input program; Lombok's javac processor transforms compiler trees. See the
[KSP overview](https://kotlinlang.org/docs/ksp-overview.html) and
[Lombok execution path](https://projectlombok.org/contributing/lombok-execution-path).
The [all-open plugin](https://kotlinlang.org/docs/all-open-plugin.html) changes
class/member openness, while the [no-arg plugin](https://kotlinlang.org/docs/no-arg-plugin.html)
adds a zero-argument constructor. Its documented reflection-only visibility must
not replace pinned compiler evidence: root's actual 1.9.23 bytecode has a public,
non-synthetic constructor, and a separate Java 17 caller compiles against it.
Check Java and Kotlin callability separately, including source and binary views;
do not infer either language's callable API merely from the plugin's name.
Pin each fixture's actual plugin/compiler versions; current documentation does
not substitute for that fixture's compiler and IDEA observations.

Root independently verified a serialization control under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/plugin-oracle/serialization`.
It uses Kotlin/compiler plugin 1.9.23, serialization core JVM 1.6.3 and JDK 17.
`Model.kt` contains only `@Serializable data class Model(val label: String)`.
With the plugin disabled, Kotlin's `Model.serializer()` fails with exit 1.
With the actual compiler plugin enabled, that Kotlin call and Java's
`Model.Companion.serializer()` compile. `javap` confirms the generated companion
method returns `KSerializer<Model>`. `compile-oracle.ps1`, source files, compiler
logs and generated class files retain the evidence. No serializer method was
written into a source stub. This proves the expected API independently; LSP
completion, diagnostics, navigation and unsaved invalidation are still unverified.

Root's additional pinned all-open/no-arg 1.9.23 and Lombok 1.18.32 controls
pass 30 compiler, bytecode and runtime assertions with JDK 17. Evidence is
`plugin-oracle/compiler-controls.py`, `compiler-controls.json`,
`compiler-artifacts.json`, the three source directories and individual logs
under the same external goal directory. With all-open disabled, both Java and
Kotlin reject inheritance/override; enabled output removes the relevant final
flags and both clients compile. Lombok disabled emits no getter or argument
constructor, and both language clients fail; enabled javac processing emits both
APIs, both clients compile and a Kotlin/Java runtime control reads the expected
values. These members were not written into generated source stubs.

The no-arg pin requires different Java and Kotlin expectations. Disabled output
has no zero-argument constructor and reflection fails. Enabled output exposes
public `()V` without `ACC_SYNTHETIC`; reflection succeeds with `isSynthetic=false`,
and a separate Java client compiles `new NeedsLabel()`. Kotlin still rejects
`NeedsLabel()` with its missing `label` argument in both modes. This matches the
[1.9.23 backend source](https://github.com/JetBrains/kotlin/blob/v1.9.23/plugins/noarg/noarg.backend/src/org/jetbrains/kotlin/noarg/NoArgIrGenerationExtension.kt),
which sets synthetic source offsets while constructing the IR constructor;
synthetic offsets do not prove a JVM synthetic access flag. The original
documentation-derived synthetic assertion fails and is preserved in
`noarg-doc-hypothesis-failed.json`, `noarg-reflect-runtime-doc-hypothesis-failed.log`
and `noarg/ReflectCheck-doc-hypothesis.java.txt`. Separate source/light-model IDEA
behavior and LSP editing are still unmeasured; neither is inferred from these
compiled-client controls.

### Actual plugin project and Neovim editing

The real mixed Gradle control uses wrapper 8.10.2, JDK 17, Kotlin 1.9.23
JVM/serialization/all-open/no-arg/Lombok plugins, javac Lombok 1.18.32 and
serialization core 1.6.3. `compileKotlin` runs before `compileJava`; both
language clients compile against actual plugin output. Sources contain annotated
declarations only, with no handwritten generated getter or serializer method.
The reusable `dev/compiler_plugin_project.lua` reproduces these six sources,
build settings and wrapper; 69 external preflight assertions check identity,
manifest hashes, edit anchors and query positions against the compiled control.
The generated portable project also passes its own offline `classes` build
(both compiler tasks execute, exit 0, 17 seconds); evidence is
`portable-plugin-classes.log`. Its first generator draft omitted the explicit
plugin-management repositories and failed identity preflight. That draft was
retained separately and the corrected fixture was generated into a new directory.

`dev/corpus/compiler-plugins.json` makes twelve first-response requests: Java
and Kotlin serialization/getter calls in complete, partial and dangling-dot
buffers. In `plugin-oracle/editor-ktlsp-fourth-1`, all twelve literal generated
method-label checks fail after completed indexing. Its one missing source remains
an independent coverage failure. Every unsaved edit and restoration is delivered
and source files stay unchanged. `editor-intellij-1` imports successfully and
passes all six serializer and three Java getter requests. Its three Kotlin
getter requests return the useful property label `name`; they retain their
literal `getName` assertion failures rather than being reported as empty results.

The same native Neovim UI probe, using `compiler-plugins-ui.json`, types a dot,
selects the actual LSP item `name` with detail `(from getName())`, accepts it
through the keyboard and verifies exactly
`fun lombokMethodFromKotlin() = Person("method").name`. No URI adapter is used
for this completion. `editor-intellij-ui-property-1/artifacts/completion-ui.json`
records one useful popup, unchanged disk source and real backend exit 0 after
shutdown. Input-to-popup handoff is 1,536 ms and RPC time 1,510 ms in this loaded
run; one sample supplies no latency acceptance. The default mixed UI cases also
pass both languages after the optional-manifest change, in
`ui-default-manifest-control/completion-ui.json`.

`plugin-oracle/ui-accepted-control/checks.json` independently compiles the actual
accepted `.name` expression against the Gradle-processed `Person` successfully.
The same expression fails against `Person` compiled with processing disabled;
its private source field alone cannot establish a usable Kotlin property.
All three processor/accepted-expression compiler controls pass. These observations
do not verify source/configuration refresh, generated-symbol navigation or
diagnostics. All-open/no-arg editing and KAPT/KSP remain separate work.

The portable regenerated project is also exercised through the complete corpus
plus native UI route with a fresh isolated official IDEA import. In
`C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-probes-20261010/pp-idea-2/artifacts`,
all twelve requests use attempt 1 and retain the same nine literal successes and
three Kotlin property-label failures. Despite corpus exit 1, the configured UI
stage runs, accepts the exact `.name` line and exits its backend cleanly with code
0. The Bash footer finishes normally; overall status 1 preserves the three
strict query failures. Harness SHA-256 is unchanged before/after, all six source
files remain unchanged, and owned IDEA/task-local Gradle processes are stopped.
This validates the optional UI route without relabeling the original assertions.

Evidence paths in this section are below
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/`.

## Measurement policy and initial UX targets

Compilation is an independent semantic oracle, not editing acceptance. The user
explicitly requires useful unfinished-code completion, live Gradle library/build
updates, and Kotlin/Java compiler-plugin behavior. Those checks must run through
the existing real-editor harness, with completion, diagnostics, navigation,
references and applied rename remaining consistent where applicable. A successful
build, a server restart that hides stale facts, or an empty fast response cannot
substitute for these checks.

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

### CEM local sources and implicit receivers: eighth checkpoint

The immutable release is
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/eighth-fixed/bin/ktlsp.exe`,
SHA-256 `20454030ec7356e8c11e1733d7a62fbd2f0bf0dfeb6fb92b0188f0903ea370e7`.
Root's raw `rtk proxy cargo test --workspace` passes 875 tests with three ignored
across 35 suites, native exit 0. All 93 inputs match before/after tests and release
compilation. `eighth-fixed/cargo-workspace-isolated.verified.json`,
`rust-release-inputs.verified.json` and `release-result.json` retain the guards.

The ordinary CEM probes exposed three independent gaps: its local NeoForge JAR
was indexed as class-name stubs despite an adjacent source JAR, Kotlin `super`
had no inferred receiver, and bare extension calls did not propagate the implicit
receiver's generic return type. The disposable CEM copy already contains both
actual archives under `build/moddev/artifacts/`; no project source was rewritten.
`ktlsp-goal-20261010/cem-boundary/` retains the archive identities, `javap`
output, compiler controls and the unchanged reduction with a same-named decoy.

Local JAR indexing now uses the exact adjacent `-sources.jar` through the existing
lazy source parser and symbol cache. Its artifact identity includes binary and
source path, nanosecond modification time and size; this is a stat identity, not a
content hash. Source-only creation, replacement and removal invalidate it, and the
editor watches that sibling even before it exists. Missing binaries cannot add
orphaned source facts. An unreadable source archive leaves indexing incomplete
instead of silently supplying shallow stubs. Statistics distinguish source files
from class stubs. The focused local tests pass 9/9; the scoped Java/Kotlin
completion and exact-source-navigation update/removal/restoration test also
passes. Their pre-fix failures and post-fix logs are in
`ktlsp-goal-20261010/local-source-pair/`.

Shared receiver/member selection now covers typed `super`, bare implicit
extensions and method type arguments without rebinding owning-class variables.
Unknown argument heads preserve ambiguous candidates; conflicting constraints
remain Unknown rather than choosing the first argument's type. Compiler-valid
single-line Kotlin class bodies remain a separate parser gap. The independent
JDK 21/Kotlin 1.9.23 controls and focused results are under
`ktlsp-semantic-gaps-20261010/cem-typed-oracle/`. The implicit-receiver suite passes
21 tests; the existing generic/member suites pass another 62. Named generic
argument mapping, conflicting-constraint joins and callable local/parameter
return types remain coverage gaps. The unchanged CEM reduction now passes all ten
semantic editor checks on the first response, including its same-named decoy,
with clean shutdown and unchanged source/binary/harness hashes. Evidence is
`cem-boundary/reduction2-eighth-2/artifacts/` under the goal evidence root; an
earlier wrapper attempt stopped before Neovim on RTK's stderr warning and remains
separate. In actual CEM, both generic-holder requests and all six incomplete
requests now pass. The two `super.getBreakSpeed` requests remain Unknown because
the real class has both a class superclass and a delegated interface. Three
missing dependency source archives remain an independent failure. Both actual
routed runs retain exit 1; [fork-corpus.md](fork-corpus.md) records the evidence.

The extension source-fact producer separately passes eight tests, and its public
store passes 22. It reads authored annotations, actual source anchors and guarded
JVM signatures from the shared index. Unsupported/generated identities and
incomplete absence remain Unknown. A known positive member can bind despite
incomplete unrelated owner coverage; unknown competing members cannot. Evidence
is `ktlsp-extension-host-20261011/producer-evidence.json`. Provider hosting,
lifecycle invalidation and an external consumer's editor results remain pending.

The seventh transformed-compiler-plugin editor run still fails all twelve
generated-API completion expectations while passing 38 controls. Actual bytecode
and compiler configuration were inspected independently; model v3 does not carry
compiler-plugin arguments or annotation-processor paths. Evidence is
`ktlsp-goal-20261010/plugin-oracle/seventh-artifact-audit/`. Binary/generated
semantic identities and pre-build editing need a shared solution, not fabricated
source locations. Processor-live diagnostic reductions also reproduce eleven
failures with one shallow-stub absence guard passing, under
`ktlsp-diagnostic-triage-20261011/`; no coverage flags were weakened.

### Gradle ownership and scoped semantic facts: fifth checkpoint

The new immutable release is
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/fifth-fixed/bin/ktlsp.exe`,
SHA-256 `c3f4f63b35ec8ff0a4df3cd41aa168cd58f670fe6faff384d5780a705d441076`.
Root's complete Rust suite passes 779 tests with three ignored across 31 suites;
release compilation succeeds. Logs are `fifth-slice/cargo-workspace-boundary.log`
and `fifth-slice/release-build-boundary.log` under the same external goal root.

Gradle snapshots now retain actual project directories, registered Java/Kotlin
source-set roots, generated/processor output roots, resolved artifact identities
and ordered per-source-set classpaths. Java and Kotlin query the same borrowed
scoped index. Type ownership is chosen before nested/member lookup; source and
shallow class-name stubs for one selected origin no longer create unrelated
version fallbacks. Reverse references and rename resolve each candidate in its
own source scope. Arc-backed origin facts avoid cloning the whole index for each
query, and model/source changes bump the shared revision.

Interactive goto/hover import seeding reads only the published snapshot. The old
request path launched a deliberately failing Gradle wrapper; the focused
regression reproduces that failure and then passes with the request fix. Source
downloads/extraction can still involve I/O; removing Gradle from this path alone
does not establish a latency budget.

An additional actual-LSP review exposed a startup/error boundary: when Gradle
resolution failed, initial broad source facts remained globally visible. Both
languages navigated/completed a closed resources-directory decoy while resolution
was pending and after failure. The immutable pre-boundary binary records eight
semantic failures, while eight same-file positive controls pass. Evidence is
`C:/Users/gummy/AppData/Local/Temp/ktlsp-semantic-gaps-20261010/pending-model-before2/`.
Known Gradle projects now start with an explicit empty scoped model; only
registered roots are scanned even when a model has no supported source sets.
Unknown ownership cannot enable a global fallback. Non-Gradle legacy behavior is
covered separately. The same external LSP reproduction passes all 23 checks
against the frozen fifth release, including all eight leak regressions and all
eight useful same-file controls. Request-path wrapper counts do not increase,
both source hashes remain unchanged and the server shuts down with exit 0.
Evidence is the sibling `pending-model-after/artifacts/results.json`.

Root's five completed Neovim scenarios are under `fifth-fixed/<scenario>/artifacts`:
basic, Java, features, semantic and mixed all exit 0. Mixed independently compiles
the original, unsaved and applied-rename snapshots, preserves exact source after
both renames, and passes both native popup/accepted-text cases with clean backend
exit. The older smoke scenarios terminate Neovim directly and log a terminated
server job; their assertion success is not a graceful-shutdown check. Each wrapper
run hashes the harness before and after execution. The catalog-only library
fixture fails both attempted navigation assertions; its eleven JDK lifecycle
checks are consequently unrun. Raw artifacts remain in `fifth-fixed/library`.
The replacement fixture compiles Gradle 9.6.1 / Kotlin plugin 2.4.0 with the
unchanged stdlib 2.2.20 pin and passes all thirteen original library assertions
on the same frozen binary. `dev/smoke_library.sh` delegates to this shared fixture.
The actual source-set root, selected stdlib version, zero unresolved inputs,
emitted `MainKt.class`, identical pinned stdlib source archive and stable harness
hash are checked in `library-fixture-proof.json`. Evidence is
`C:/Users/gummy/AppData/Local/Temp/ktlsp-gradle-model-20261010/fifth-library-actual-gradle/artifacts/`.
This adds the sixth scenario, for 255 assertions plus two native UI samples.
The scoped policy and all thirteen library expectations are unchanged.

Actual custom-root evidence is `source-root-{cold,warm}.log`: JDK 17, Gradle
8.10.2 and Kotlin 1.9.23 compile ordinary and late custom Java/Kotlin roots,
include the configured processor-output root, and exclude resource directories
containing deliberately invalid Java/Kotlin decoys. Both strict configuration-cache
runs exit 0 and the warm run reuses the cache. The 194-module Gradle dump's older
configuration-cache reuse evidence predates this resource-root refinement and
is not substituted for the newer custom-root control.

The live reload acceptance remains failing. The strengthened existing
`KTLSP_GRADLE_RELOAD=1 ... gradle-live` probe has 30 passing and 42 failing checks
out of 72 in
`C:/Users/gummy/AppData/Local/Temp/kr-reload-baseline-4/artifacts/`.
All four real offline builds compile; initial classpath checks and unsaved
cross-language controls pass. New/updated/removed libraries and standard/custom
generated sources remain stale on the same connection. A negative removal check
requires its prior useful positive result. Refresh implementation follows this
ownership checkpoint; no restart-based run counts as live refresh acceptance.

### Imported superclass identity: fourth checkpoint

Remaining superclass readers now use the same typed, instantiated hierarchy for
completion, callable selection, navigation and diagnostic completeness. Recovered
incomplete-file declarations replace only the matching package/container identity.
An unresolved superclass, cycle or depth cutoff retains an Unknown observation;
it cannot prove that a member is absent while hierarchy inputs are incomplete.

Root's `rtk cargo test --workspace --no-fail-fast` passes **746 / 3 ignored**
across 30 suites. Targeted Rust formatting and `git diff --check` pass.
Evidence is `C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/fourth-slice/`:
`cargo-workspace.log` and `release-build.log`. The frozen release is
`fourth-fixed/bin/ktlsp.exe`, SHA-256
`8b9166c05d7769e8a3cecbfe18dcfad89acc7a0bf9fb96d51b925b08981a8c0c`.

The existing Java scenario now passes **99/99**; its original 65 expectations
remain. Running the expanded scenario against the third release gives 93 pass /
6 fail: four completions choose the wrong superclass, and two Kotlin inherited
calls navigate to it. Fourth-release mixed **36/36** and semantic **18/18** also
pass. Root inspected all three summaries under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-semantic-gaps-20261010/`:
`directsuper-fourth-fixed`, `mixed-fourth-fixed`, `semantic-fourth-fixed`.
All 16 superclass requests record attempt 1 after outbound document changes.
Valid complete caller diagnostics must actually arrive and be empty; switched
complete callers must report exactly two argument mismatches. Compiler controls
on the six generated files independently verify original and switched-valid
superclasses, and reject the switched-invalid calls with exactly two errors per
compiler. See `compile-generated-directsuper.ps1` and
`compiler-generated-directsuper/{original,switched-valid,switched-invalid}`.
Mixed original, edited and applied-rename compiler snapshots all exit 0.

The `mixed` harness now also runs `dev/nvim_completion_ui.lua` in Neovim's input
loop. Builtin autocompletion receives actual insert-mode `.` input in both
languages. `popupmenu_show` must contain the intended LSP member and exclude the
other receiver's member; keyboard selection must produce the exact expected
buffer line. Protocol time, input-to-menu handoff and selection-to-text
verification are recorded separately. Source files remain unchanged, and clean
shutdown requires acknowledgement, an exit notification and process exit 0.

The full fourth-release mixed run passes its original 36 checks and three
compiler snapshots, then both languages' UI controls. The independent IntelliJ
run passes the corresponding 38 RPC controls and three compiler snapshots; its
final UI-only run also passes both languages. Each final UI control uses two
cycles per language, four useful first responses and four exact accepted lines.
Evidence under `C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/` is
`mixed-ui-integrated/artifacts/`, `mixed-ui-final-control/completion-ui.json`,
`mixed-ui-intellij/artifacts/` and
`mixed-ui-intellij-ui4/completion-ui.json`.

Earlier UI setup failures are retained separately: enabling autotrigger after
an existing disabled handle did not replace Neovim's original options; typing a
Java semicolon before asynchronous `completionItem/resolve` completed invalidated
Neovim's pending edit by changedtick. The probe now reenables the handle with the
requested options and waits for resolve/server-command completion before
inspecting text. The IntelliJ Kotlin item actually inserts through a server
command and `workspace/applyEdit`; this path is exercised without changing its
handler or weakening stale-edit checks.

These are small three-file controls with other investigations running, **not
performance acceptance**. Cycle 1 is first-after-readiness, cycle 2 is one warm
repetition per language; neither is a latency distribution. The popup event
measures the editor's menu handoff, not physical screen pixels. Repeated real-root
samples and normal editor debounce remain required.

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

### Imported superclass identity: original reproduction

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
before repair). The fourth checkpoint above verifies the repair and adds the
editor acceptance case. This original external probe retains the independent
false-diagnostics reproduction; the older editor fixture itself did not show
that diagnostic failure.

### Next reproduced gaps: full-index source identity and call context

Moshi's old startup observation passes three semantic checks, but after project
and library indexing, only its member completion passes. `JsonAdapter.Factory`
navigates to seven unrelated nested `Factory` types and omits the intended one;
`reader.beginObject()` navigates to both project 1.15.2 and dependency 1.12.0
declarations. The same indexed IDEA probes pass 3/3. Retrofit also returns project
2.11 and dependency 2.9 targets together, and `Response.body()` includes both a
private field and a method from both versions. These are correctness failures,
separate from missing source coverage, and are not accepted as ambiguous source
navigation. External evidence is under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-probes-20261010/`:
`moshi-1.15.2-ktlsp-indexed-2/artifacts/corpus-requests.jsonl` and
`retrofit-2.11.0-ktlsp-indexed-2/artifacts/corpus-requests.jsonl`.
Next reduce owner-qualified nested types, callable-vs-field context and
module-specific source/classpath selection into focused regressions.

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
