# Fork acceptance and evidence

Updated: 2026-10-12 (Asia/Tokyo). Goal remains **in progress**.

## Scope

Deliver a fast, shared Java/Kotlin editing experience in Neovim for Minecraft mod
development. Acceptance requires useful and correct completion, inference,
diagnostics, navigation, references and rename across both languages and unsaved
buffers, using the same Gradle dependencies and generated sources.

Provide a public analysis extension boundary for a separate Mixin LSPExtension project.
The extension must read shared semantic results and contribute facts used consistently
by existing features. A small representative Mixin test may prove that boundary;
shipping a Mixin implementation is outside this project's scope. Source and extension
configuration changes must invalidate contributed facts and cached results.

The real-project corpus is CreateEnchantableMachinery (CEM), excluding Mixin code
from acceptance probes, plus five additional distinct Java/Kotlin projects. Small
generated fixtures supplement that corpus; they do not replace it.

Overall percentage estimates are withdrawn: implementation and acceptance were mixed
in the earlier estimate. Report verified behavior and remaining cases per project.
Do not infer a project's analysis failure from a run timeout, unavailable source
archive or cleanup failure, or collapse those observations into a "0/6" score.
Focused fixtures also do not establish whole-project IDEA parity.

Acceptance concerns useful and correct analysis during editing: unfinished code,
unsaved Java/Kotlin changes, dependency/build updates, compiler plugins and consistent
extension facts. Compare responsiveness and memory with IDEA under the same host,
project state and cache conditions. A deadline bounds an experiment; if it prevents
requests from running, those requests are unmeasured. Investigate a repeatable server
stall as a product issue separately. A missing source archive fails a navigation case
only when that case requires its source and cannot resolve correctly. Historical
harness exit codes below include broader operational checks and are not analysis scores.

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
command or an advertised capability is not a semantic pass. A valid RPC must satisfy
the expectation; reviewed unknown-receiver controls may expect an empty result alongside
a useful known receiver in the same edited state.

## Acceptance matrix

| Case | Expected result | Observed result | Status | Evidence / next action |
| --- | --- | --- | --- | --- |
| Rust baseline / current slices | Workspace tests pass with useful regression expectations | Current selected-compiler worker passes 1,285 / 0 failed / 4 ignored; focused Kotlin 67/67; formatting/release native 0. Prior classpath 37/37 and generic interoperability 43/43 | Current Rust, formatting and release verified | Checkpoints 39–41; Java wildcards, where/F-bounds, arbitrary compiler plugins and full-project parity remain open |
| Real Neovim baseline | Starts server, sees capabilities, resolves asserted targets | Current basic 9/9, features 78 matched / 2 unmet, library 13/13, Java 194 matched / 5 unmet, semantic 107 passing assertions. Saved Java publication probe previously passed 99/99 | Partial; same seven ordinary expectations remain open | Checkpoints 38/39 distinguish semantic RPCs, setup checks, versionless diagnostic correlation and operational shutdown. `native-shape-editors/results.json`; checkpoint 36 measures owned Moshi server working sets, editor memory remains unmeasured |
| CEM project model | Minecraft, NeoForge, Create and Kotlin classpaths and sources are visible | Twentieth indexes 101 dependencies with zero failed inputs and two missing sources. All seven strict first-response completion/goto assertions and native popup acceptance pass; the whole accepted buffer compiles. Error Prone annotations and Gson now load; missing Patchy and KotlinForForge sources retain a broader harness exit 1 | Focused queried semantics and accepted code verified; broader cases open | `ktlsp-cem-twentieth-fixed-1-20261011/{run-cem-first,actual-accepted-compiler}/result.json`. Full CEM diagnostics, overrides, required missing-source navigation cases and performance remain |
| Corpus selection | Five additional real, diverse mixed Java/Kotlin repositories with pinned revisions | OkHttp, Moshi, Retrofit, Spring Framework and Gradle pinned and clean; concrete ordinary Java/Kotlin edges identified | Discovery complete | [fork-corpus.md](fork-corpus.md); semantic and performance acceptance still pending |
| Ordinary completion | Correct receiver members, annotation names and valid override proposals in Java/Kotlin | Java `h.as` baseline empty; first fix returns useful members through shared queries; focused Java/Kotlin completion tests pass 143 with 1 ignored | Partial | Verify release editor behavior; annotation/override smart proposals remain pending |
| Incomplete editing | Useful context-aware completion in unfinished Java/Kotlin, including syntax errors, partial names, missing delimiters and unresolved argument types | Earlier CEM, Retrofit, Moshi and OkHttp each match all 6/6 first-response requests. Current Spring and Gradle each match 6/6; Gradle also matches all 3/3 golden requests after repairing model transport | Partial; focused cases observed on all six roots, wider editing behavior remains open | Checkpoints 33 and 35 and [fork-corpus.md](fork-corpus.md). A paired useful known receiver is required for negative cases; these cases do not establish whole-project parity |
| Inheritance and generics | Correct inherited members and substituted types across both languages | Structured Java type arguments and typed superclasses resolve mixed completion/goto both directions. Fourth release repairs imported superclass collisions across completion, goto and diagnostics; unsaved import switches are fresh on the first response. Growing cycles are bounded and incomplete hierarchy facts remain Unknown | Partial; focused repairs verified | Java 99 includes 34 new editor assertions. Method inference, bounded wildcards, declaration shadowing and real-project coverage remain |
| Overloads and nullability | Select correct overloads; preserve nullable/platform types; no arbitrary ambiguous target | Tenth-fixed-2 known Java Int/String calls share typed selection across navigation, result inference and references. Strict typed override dominance preserves distinct overloads, boxed signatures and unknown competitors. Phased Java widening/boxing specificity remains Unknown | Partial; current focused/editor hierarchy controls verified | Related eleven suites pass 205 tests, including 58 member-selection and 27 JVM projection cases. Measure wider compiler/IDEA cases |
| Kotlin extensions | Visible compatible extensions resolve and complete, including chained results | Retrofit's six golden/six incomplete requests pass. Current Spring reduction repairs four failed requests; real Spring passes all nine original/additional first-response queries, including explicit-this and dangling selectors | Focused repair verified; full-project acceptance partial | Checkpoint 29 below. Real Spring still has ten missing dependency source inputs and a 15-second forced shutdown |
| Function-local Kotlin extensions | Respect declaration order, lexical scope and typed receiver/overload selection across features and unsaved edits | Real Neovim local checks improve 31/44 to 44/44, preserving the previous 65 checks. Current normalized receiver arguments and inline nominal bounds have shared regression controls; ordinary semantic scenario retains 107 assertions | Focused semantics verified; where/F-bounds and projected receivers remain coverage gaps | Checkpoints 33/39: exact references, applied whole-file rename, signature/hover, type-definition and inlay hints have Rust regression controls; no six-project parity or speed claim |
| Unsaved cross-language edits | Both languages immediately observe edited declarations and types without saving | Release mixed fixture: both directions complete and navigate on their first request after both changes are outbound; opened JDK source rename/delete no longer leaves stale facts | Partial; focused repair verified | Normal Neovim cross-buffer debounce observation is separate; actual corpus and repeated edit stress remain pending |
| Rename | Applying workspace edits updates the intended Java/Kotlin declarations and usages only | Tenth-fixed-2 applies the complete six-site override family, preserves all five expected buffers and passes actual Java/Kotlin compilation plus 33 bytecode assertions. JVM facade exported-alias/authored renames and external consumer applied renames also pass | Focused fixtures verified; wider corpus and inherited implementation edge pending | `ktlsp-goal-20261010/tenth2-java-rename-compiler-1/`, `retrofit-jvm-editor/portable-release-tenth2-1/artifacts/` and `ktlsp-shadow-tenth-fixed-2-20261011/editor/`. Ambiguous or read-only family members refuse partial edits |
| Dependencies and generation | Resolve real Gradle dependencies and generated sources, with correct module visibility | Sixth release repairs the Gradle 7 regression: Moshi passes 3/3 golden and 6/6 incomplete requests. Gradle retains 3/3 exact golden targets. All six model-v3 snapshots have complete source sets and zero unresolved entries; five roots still lack dependency sources | Partial; model repair verified | [fork-corpus.md](fork-corpus.md) records all twelve immutable runs, unchanged inputs, exact remaining semantic gaps and independent missing-source failures |
| Live Gradle changes | An already-running client reflects dependency additions, version changes, removals and new build outputs | Current native automatic-watching lifecycle passes all 86 original checks, including Kotlin's deleted oldApi diagnostic. Four real builds, completion, exact definitions, generated-source changes, dirty buffers and clean shutdown pass | Focused automatic-refresh semantics verified; wider transforming cases open | Checkpoint 41: native-worker-canonical5/editor/artifacts under the selected-compiler slice; model v7, native watches and zero explicit notifications. Positive Kotlin result has transmitted-text SHA and accepted-store generation proof |
| Kotlin compiler inputs | Capture and use the task's actual source selection, ordered libraries, compiler runtime and selected JDK across refresh | Selected 1.9.23/2.4.0 runtime recipes match independent probes. Actual private-overlay diagnostics report unsaved errors; 1.9 also clears a repaired new buffer and preserves UTF-16 positions after Japanese/emoji text | Captured inputs and focused unsaved diagnostics verified | Checkpoints 40/41; opaque plugin options, other hosts/source sets, transformed/generated APIs and general producer closure remain open |
| Compiler plugins and processors | Analyze generated and transformed APIs used from both languages, including unsaved source changes | Current path-cache saved Java/Lombok probe retains 99/99: first getter/typed chain, four observed native builds, config/dirty retraction. Earlier KAPT/KSP 102/114 retains twelve stale/removed diagnostics; original mixed twelve editing cases remain OPEN | Partial; narrow saved Java publication verified, closure/mixed/unsaved useful completion unresolved | `ktlsp-path-cache-slice-20261011-1/native-java-1/editor-root-1/artifacts/compiled-java-results.json`; compilerClosureComplete=false throughout. Source-less navigation stays unavailable; no producer-absence guard is relaxed |
| Extension boundary | Separate consumer reads semantic facts and contributes consistent enabled/disabled results | Sixteenth external Mixin consumer passes 140/140 actual LSP checks, with zero failed/missing dependency inputs. Completion, diagnostics, navigation, exact references, applied rename, enabled/disabled/configuration and unsaved-type controls pass | Focused extension contract verified; general generated APIs and loading ABI remain open | `ktlsp-shadow-sixteenth-replay-20261011/final-editor-proof.json`. Twenty-nine first responses, nineteen delivery barriers and four applied/inspected renames. Actual server exit zero, one acknowledged shutdown and no forced termination |
| Independent IDEA comparison | Expected member/type/navigation/diagnostic behavior verified in IDEA for acceptance cases | Shared mixed fixture: adapted IntelliJ LSP 38/38 pass and fixed ktlsp 36/36 pass, including strict first responses after change delivery. Earlier ktlsp had seven semantic failures | Shared fixture verified | IDEA has two additional import/readiness checks. Windows URI/version adapter is explicit, vanilla plugin has an applied-rename failure; actual corpus parity remains pending |
| Correctness plus latency | Useful responses stay responsive, with latency and memory compared against IDEA under matched conditions | Earlier shared snapshot/path API allocation improvements remain verified. Real Moshi has 31/31 correct native UI cycles per engine: warm input-to-popup p50/p95 43.927/48.219 ms for ktlsp and 453.308/763.453 ms for IDEA; post-readiness server working-set medians 854.0/1,892.0 MiB | One real-project editing comparison verified; wider comparison pending | Checkpoints 28 and 36. Memory excludes editor/Gradle children and samples active editing, not idle steady-state; broad cold/warm project comparisons remain |

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

### Successful compiler output and dependency readsets

The current immutable fixed-2 plugin replay retains **38 PASS / 12 FAIL** checks.
All twelve failures are transformed-API completion: Java/Kotlin getter and
serializer, each in golden, partial-member and dangling-dot states. All sixteen
delivery/restoration controls pass, with no readiness failure. Evidence is
`ktlsp-compiled-freshness-plan-20261011/baseline-evidence.json`; no compiled-API
host adapter has shipped.

A separate controlled experiment runs thirteen same-module compiler scenarios:
63 native commands, 59 successful and four expected failures. Nine successful
direct compiler receipts and four rejected incomplete-module receipts preserve
exact source, output, plugin, processor, toolchain and ancestor-config inventories.
Valid consumer changes preserve the target classes. Failed javac can still emit
`Person.class` and leave successful Kotlin outputs; those partial outputs do not
certify a successful whole-module build. Changing `Model.label` from String to Int
leaves `Model$Companion.class` byte-identical while changing Model and its serializer.
Adding an ancestor `lombok.config` changes `getName()` to `name()` without changing
Person's source. Class content alone therefore does not establish producer freshness.

The next host slice must reuse the existing provider/read-session contract and
track actual owner source plus successful output, selected origin, plugin/config
and negative ancestor reads. Generated members retain binary identity without
invented source definitions. Unknown producers retain whole-input validation;
a narrower positive projection requires an explicit verified producer contract.
The experiment uses a four-source reduction with pinned serialization/Lombok
plugins, not the original complete six-plugin Gradle configuration or a production
Gradle task-completion receipt. It supplies no editor improvement or general
processor-isolation proof. Evidence is
`C:/Users/gummy/AppData/Local/Temp/ktlsp-plugin-readset-proof-20261011/{readset-report.txt,results.json,commands.json}`.

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
they are **provisional engineering targets**, not universal pass/fail thresholds,
measured results or a claim of IDEA parity. Record host load and cache conditions;
use matched comparisons to distinguish implementation cost from environmental delay:

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

The earlier steady-RSS/peak-commit/client-memory limits (3 GiB/4 GiB/128 MiB) and
two-second shutdown target were agent-selected investigation targets. They are
withdrawn as acceptance gates. Continue measuring actual memory and investigating
excessive allocation, watch scaling and reproducible stalls. Report cleanup outcomes
separately; they neither invalidate a completed semantic assertion nor prove that
an unexecuted assertion works. Host resource abort guards remain execution safeguards.

An isolated Gradle follow-up exposed a client-memory scaling failure: 8,796 watch
patterns took about 88 seconds to register, with Neovim lifetime peak commit
63,475,847,168 bytes and ktlsp peak commit 5,982,179,328 bytes. This was the ktlsp
client, not IDEA LSP. The owned process tree was forcibly stopped for host resource
pressure; the run supplies no performance acceptance. See the separate measurements
and provenance in [fork-corpus.md](fork-corpus.md).

The next isolated replay uses provisional abort guards: Neovim private bytes above
1 GiB, ktlsp private bytes above 8 GiB, system commit headroom below 8 GiB or physical
available memory below 4 GiB. These protect the host while investigating scaling;
they are not UX memory budgets or evidence that the server's current footprint is
acceptable. Registration cardinality, event classification cost, index storage and
request algorithms must be assessed alongside semantic correctness.

Record baseline-derived budget decisions before accepting performance changes.
Report failures and missing semantic results separately; fast empty responses cannot
lower the accepted latency distribution. Sampling/profiling runs are separate from
timing runs. Initialization timing never substitutes for editor completion timing.

## Work log

### Parser termination and compiler inputs: ninth Rust checkpoint

The scanner failure has an existing upstream report, [issue #10](https://github.com/tree-sitter-grammars/tree-sitter-kotlin/issues/10),
and an unmerged fix, [PR #11](https://github.com/tree-sitter-grammars/tree-sitter-kotlin/pull/11).
The current upstream default and the independent fwcd main revision were also tested;
neither repairs the exact Source/KSP reductions. The newer fwcd parser also has a
different AST contract, so its consumer-contract failures are not all grammar bugs.
Using PR #11's old base directly repairs the inline bodies but regresses the existing
primary-constructor-newline and context-parameter controls. Keeping our previous
`10f5277602bc3af132bfe21997b775d0ac0a58bf` base and cherry-picking the original fix
preserves both. The dependency now pins
[`42f93dc3e3d73ef07a657fa5fd557e39c65704ee`](https://github.com/cotrin8672/tree-sitter-kotlin/commit/42f93dc3e3d73ef07a657fa5fd557e39c65704ee),
with the original author and commit attribution retained; no Cargo checkout was patched.

The independent engine-0.24.7 causal comparison passes 13/13, including the original
issue's adjacent-class example. The grammar's existing 27 corpus cases pass before
and after; its added regression fails before and passes after, giving 28/28 on the
published shipping source. Tree-sitter CLI 0.27.0 corpus checks are recorded separately
from the actual Rust engine checks. Product tests repair five previously failing
Source/KSP cases and now pass all six, covering Java/Kotlin completion, exact navigation
and unsaved replacement/removal. Another 86 inference/member/generic checks pass.
The external 92-test run has unchanged hashes for all 96 then-current product inputs.
Evidence is under `ktlsp-semantic-gaps-20261010/{grammar-comparisons,grammar-corpus-pr11,grammar-integration-pr11}/`.
Root's direct locked-dependency six-test result is
`ktlsp-goal-20261010/compiler-metadata/parser-shipping-2.{out,json}`.
Bodyless annotation declarations remain a separate parser gap. The library symbol
cache and ktcheck durable cache versions were bumped to discard old parsed facts.

Unreadable UTF-8 entries previously allowed a source archive containing one good and
one bad file to publish and cache a partial success. Source-archive and JDK-shard reads
now return an error atomically; failed worker batches do not publish partial facts or
write a success cache. The three focused cold/warm/repair and worker checks pass after
their baseline failures, under `ktlsp-goal-20261010/local-source-pair/partial-source-*`.
This is read-error coverage; parser recovery, source-count limits and directory-read
completeness have not been proved by those checks.

Gradle model v4 captures configured Java/Kotlin compiler task identities, serialized
arguments, annotation-processor paths, available toolchain information, options and
output directories. Arguments use JSON escaping, including tabs, newlines and Unicode.
Unsupported task adapters remain explicitly incomplete and retain known source ownership.
This is compiler configuration, not evidence that producers are closed or class files
match the current source. JVM task-name discovery is a bounded adapter; other compilation
models and processor paths encoded inside opaque options remain unknown.
The classpath unit suite passes 21 tests. Actual Gradle 7.4.2/Kotlin 1.8.21 KAPT/KSP
exports 16 task records, and Gradle 8.10.2/Kotlin 1.9.23 compiler plugins export four.
Both pass strict configuration-cache checks and reuse; 38 and 21 source/build/class
inputs respectively remain unchanged. Moving the dump action out of its enclosing
Project closure also repairs a reproduced pre-existing Gradle 7 configuration-cache
failure. Evidence is `ktlsp-goal-20261010/compiler-metadata/{old-head-control-1,old-6,modern-2}/`.
Generated binary APIs and automatic watching of all compiler-only inputs remain pending.

The first complete ninth workspace attempt is deliberately retained as failed:
631 pass, five fail, two ignored before Cargo stops, native exit 101, with 97 unchanged
inputs. Its `explain_resolution` failures expose missing accumulated incompleteness
reasons and fixtures that had not supplied the new compiler-producer proof. Shared
reason accumulation now preserves compiler, project, library and annotation-owner
reasons together; known declarations still resolve first. Closed-world test fixtures
supply the compiler proof before scanning, without erasing the package/source-set facts.
The focused rerun passes 396 ktlsp tests and 110 core tests with one ignored, including
all seven original explanation cases and three added controls. A new immutable full
run is required before a ninth release is accepted. Evidence is
`ktlsp-goal-20261010/ninth-explain-reasons/`. The earlier
direct parser attempt failed before tests because a referenced Rust 1.99 toolchain's
standard library was unavailable; retrying with the installed baseline Rust 1.98.1
passes. Verification scripts now select that toolchain explicitly.

The second complete run passes 930 tests with three ignored across 38 suites,
native exit 0 in 30.906 seconds. Release compilation exits 0 in 40.086 seconds;
all 97 Rust/build inputs match across both runs. The immutable server is
`ktlsp-goal-20261010/ninth-fixed-2/bin/ktlsp.exe`, SHA-256
`e4b726b8920be45ec06941c71c1fc5955bc327c393487ad9d3098b738e7e8ad1`.
The sibling `rust-release-inputs.verified.json` confirms source and binary hashes.
These are test/build durations, not editing-performance measurements. The next
checks use the unchanged real-CEM corpus and `KTLSP_PROCESSOR_RELOAD=1` Neovim route;
their ninth results have not yet been claimed here.

Registered Rust analysis providers now receive normalized accepted source facts
and publish validated bindings into the shared index. Completion, inferred member
metadata, navigation, references and applied rename use those relations. Configuration,
model/source edits, failure, disable and removal invalidate publications; providers
are registered through the public host constructor, with framework policy outside
ktlsp. Focused provider/producer/reference/rename checks pass 24 cases, and the full
suite above includes the host tests. A separate actual-Mixin consumer compiles, but
its five-feature Neovim lifecycle proof is pending. Paired external Java/Kotlin probes
also expose a remaining late-ownership bug: a skipped open-buffer dependency batch
can change scope visibility without scheduling provider normalization. That failure
is retained under `ktlsp-extension-host-20261011/late-owner-before-paired.txt` for the
next repair; ordinary CEM runs do not register that consumer.

### Ninth editor rechecks and late-ownership repair

The ninth source is committed as `2b2325e`. Its unchanged real-CEM manifests now
pass all four golden completion/exact-definition requests and all six incomplete
requests (five editing cases), each on its first response after both indexing
phases. Both runs retain exit 1: dependency completeness reports four failed
archives and three missing sources. All queried/edited/declaration source bytes,
helpers and the frozen binary remain unchanged. Evidence is
`ktlsp-corpus-probes-20261010/cem-working-copy-ktlsp-ninth2-{golden,incomplete}-1/`.
The archive audit identifies three readable compatibility archives containing
only `module-info.java`; calling these IO failures is incorrect. Commons Compress
1.26.0 contains one non-UTF8 Java comment in an otherwise readable archive. Its
unproven encoding remains an explicit input failure, with no guessed decoding.
Raw entry/CRC/content hashes are in
`ktlsp-goal-20261010/source-archives-cem/ninth-cem-and-archives.json`.

The same-client KAPT/KSP rerun restores all previously failing unsaved-member
completions. Real builds, current API completion, exact generated-source
navigation/removal and buffer preservation pass. It still fails twelve diagnostic
expectations for obsolete or removed APIs; no expectations were weakened.
Evidence is `ktlsp-goal-20261010/processor-refresh-ninth-1/`.
The broad Java editor recheck also retains five missing-diagnostic failures
(unresolved reference, arity, argument type and two changed-superclass cases).
These are coverage gaps from incomplete producer knowledge, despite passing
ordinary completion/navigation/refactoring controls. Evidence is
`ktlsp-goal-20261010/ninth2-java-1/`; its legacy forced shutdown does not prove
graceful termination. None of these correctness runs supplies timing acceptance.

The late-owner repair marks provider inputs dirty only when file-origin or scope
metadata actually changes. An open buffer still rejects stale symbols; repeated
unchanged metadata and old generations do not trigger an evaluation. Paired
Java/Kotlin tests change from two failures to two passes; the related Index, host,
source-fact, reference, applied-rename and scope suites pass 50 tests. Full fmt and
Lua syntax checks pass. `ktlsp-extension-host-20261011/late-owner-stable.json`
records source hashes. The opt-in external Lua corpus hook reuses the existing
client, checks, source guards and shutdown. Actual external-provider editor
acceptance and a fresh full workspace run remain pending for this repair.

### External consumer and current IO/JVM work

The late-owner repair is committed as `64565d0`. A separate consumer uses a
283-file immutable `git archive` of that revision, archive SHA-256
`36dd0c3a7b0fcb795280b21607bb61e1e30013d25f4aa1a280d6960faa68aaf1`.
Its full workspace run passes 932 tests with three ignored across 38 suites.
The consumer release is SHA-256
`da14d159c55bdee916af1a76ae1b5806498c5f2f2df46f65899b7190f01b3851`.
All product, original/copied consumer, fixture, probe and binary guards remain
unchanged. The existing Neovim corpus route returns 125/140 checks passing,
native exit 1, with graceful shutdown and nineteen delivered-version barriers.
Enabled/disabled controls cover completion, diagnostics, navigation, references
and applied rename; alias literal/authored references and whole-buffer rename
checks pass. This is actual host integration, not complete extension acceptance.

The prefixed Int shadow remains Unknown because its String overload has an
unknown source signature. A separate diagnostic binary against the same frozen
product proves that exact cause without modifying the original proof binary:
the Int candidate is `(I)I`, while the competing String candidate is
`UnsupportedSource`. Parsing both actual JDK 17 `java.lang.String` and Kotlin
1.8.21 `kotlin.String` reproduces the ambiguity; either source alone and both
explicitly qualified controls resolve. Preserve the competing-candidate guard
and repair shared intrinsic type identity. Two failed dependency archives and an
ordinary String navigation result remain separate failures. Evidence is under
`ktlsp-shadow-64565d0-20261011/{editor,diagnostic-editor}/`.

CEM's unchanged ninth model records four complete configured compiler tasks:
Java/Kotlin compilation for both main and test, with no metadata problem flags.
This proves configured input discovery, not that outputs match current inputs.
The safe field/count-only audit is
`ktlsp-goal-20261010/compiler-metadata/ninth-cem-compiler-metadata.json`;
opaque compiler argument values were not copied into this report.

The current source-IO repair propagates errors through library/local loaders and
their callers instead of interpreting every empty result as failure. Readable
empty archives remain valid; absent, corrupt, class-bearing, Kotlin-metadata,
nested-archive and non-authoritative fallback artifacts cannot prove empty source coverage. Source
directory enumeration and decoding errors reject the entire batch. Focused
ktlsp library tests pass 213 with one ignored, and the navigation suites pass ten
with one ignored. `compiler-metadata/source-io-focused-all.out` and
`source-io-navigation-tests.out` retain those runs. The later artifact-stat guards
are included in the input-matched tenth full run and release/editor checks below.

Review added strict controls for local metadata-only/nested archives, lost lazy
source archives, and an unrelated empty default Maven artifact. The first two
guards reproduce 0/2 passing before the repair; all three coverage guards now
pass. Empty coverage uses every model-selected binary and its captured stat
identity before and after inspection. Marker-only or partially materialized lazy
directories cannot replace an unavailable archive. Local empty results retain
their explicit origin, and a stub cache miss repeats extraction rather than
trusting a possibly interrupted directory. The additional partial-stub repair
case also passes in the tenth full run. Evidence is
`compiler-metadata/source-io-coverage-{red-2,green}.out`.

Shared applicability now selects known Java Int/String overloads consistently
for navigation, result inference, diagnostics and references. Unknown argument
types retain competing candidates, and a unique invalid call can navigate to its
declaration while supplying no invented result type. Portable tests apply and
inspect authored and exported-alias renames with same-named decoys. The final
member/JVM suites pass 70 tests; the earlier affected suites pass 510 with one
ignored, and core passes 150. Java widening/boxing remains conservative Unknown
pending phased overload specificity. Evidence is
`ktlsp-shadow-64565d0-20261011/java-overload-{focused,final,core}.txt`.

The pure classfile decoder separately passes 126 tests (110 existing core and
sixteen decoder cases), with unchanged hashes for twelve actual compiler outputs.
Seven portable fixture classfiles preserve Lombok getters, serialization
signatures/bridges, no-arg on/off constructors and Unicode declarations. The API
retains origin plus binary content SHA, raw descriptors/flags/signatures and
attribute presence. It creates no source location or language-callability proof.
Code and nested annotation payloads stay opaque, and malformed/bounded-input
guards are tested. Evidence is
`ktlsp-classfile-groundwork-20261011/decoder-verification.json`. Generated API
publication, output freshness, Kotlin metadata and live editor checks remain.

The Commons Compress 1.26.0 failure has a declared encoding oracle: its exact
parent `commons-parent:66` configures javac with `commons.encoding=iso-8859-1`.
All 404 Java entries pass CRC and lossless ISO round trips; strict UTF-8 rejects
the `0xF6` byte in `TarArchiveOutputStream.java`. Actual javac 17 succeeds with
the declared encoding, fails with UTF-8, and succeeds after lossless UTF-8
normalization with an identical compiled-class SHA. Two other files contain
UTF-8 spellings in comments, so this establishes the compiler's declared
interpretation, not a guessed encoding or uniform author intent. No product
decoding policy has changed. Evidence is
`ktlsp-source-encoding-20261011/{archive-audit,javac-controls}.json`.

KFF's outer archive has no top-level classes but contains eight nested JARs.
Each matches a separately selected binary in both CEM source sets by SHA.
That observational containment proof is not yet a product completeness rule;
the conservative nested-archive guard retains its missing-source status.

The source-backed JVM facade probe has a portable fixture and uses the existing
`corpus` route. The guarded tenth release passes all 81 assertions, including
closed-file first responses, delivered unsaved versions, exact references and
inspected whole-buffer renames. Original and both actual renamed snapshots pass
all nine Kotlin/javac/runtime controls. Evidence is
`ktlsp-semantic-gaps-20261010/retrofit-jvm-editor/portable-release-tenth-1/artifacts/`.

The tenth frozen product passes **998 tests, zero failures and three ignored**
across 40 suites, native exit 0. Fmt and release compilation also pass; all 270
product inputs, including seven binary fixtures, are unchanged across the run.
The immutable `ktlsp.exe` SHA-256 is
`342dafb060cb7912b58b32a105bb3016f1ca64ae759ec8e926b87e8f95c01ee8`.
Evidence is `ktlsp-goal-20261010/tenth-fixed-1/result.json` and its raw logs.
These checks establish this frozen source slice, not later edits.

The external consumer copies the same 270 guarded inputs and passes **139/140**
checks, native exit 1. Its original 14 semantic failures all pass without changing
expectations. Completion, pull/pushed diagnostics, exact definitions/references,
applied rename, configuration changes, dirty Int-to-Long updates and restoration
pass, with nineteen delivered-version barriers and graceful shutdown. The sole
remaining failure is dependency coverage: kotlin-stdlib-jdk7/jdk8 1.8.21 contain
only module descriptors. Their conservative classification is unchanged.
Evidence is `ktlsp-shadow-tenth-fixed-1-20261011/{editor,missing-sources-audit.json}`.

Actual CEM retains all **4/4 golden and 6/6 incomplete** first responses; actual
Retrofit now passes all **6/6 golden and 6/6 incomplete** responses, including
the paired useful/unknown receiver control. Both roots still fail their strict
coverage assertion. [fork-corpus.md](fork-corpus.md) records counts, guards and
the original failed Retrofit controller attempt separately. These concurrent
correctness runs supply no latency acceptance.

The broad Java replay finds a new incoming-call failure for `Helper.assist()`:
the implementing class method and `Worker.assist()` both survive selection,
so reference resolution drops the call. Three focused hierarchy tests reproduce
this alongside generic overrides and retained inherited overloads. Five existing
missing-diagnostic failures remain independent producer-closure gaps. Evidence
is `ktlsp-goal-20261010/tenth-java-1/artifacts/java.out` and
`ktlsp-shadow-tenth-fixed-1-20261011/override-red.txt`.

Review of that new hierarchy control also exposes unsafe ordinary rename:
renaming only the implementation declaration and its statically bound calls
leaves the interface contract unchanged. Its applied snapshot is not a valid
rename success. The regression now requires the complete related declaration
and call family; distinct overloads, static hiding, private declarations and
unknown hierarchy/signature facts must not be guessed into that family.
This is separate from the passing JVM exported-alias and provider rename probes.

The existing `java` editor scenario now includes a public five-file contract
fixture. It requires both declarations and four Java/Kotlin calls to change,
applies those exact six edits in Neovim, compares every complete buffer, retains
the same-name overload/unrelated method, verifies delivered changes and fresh
Kotlin navigation, and preserves all disk files. Against tenth-fixed-1, the
server returns only the implementation declaration edit; the new probe records
eleven failures and refuses unsafe application. The other six new controls pass.
No Lua errors occur, and helper/binary inputs remain unchanged. Evidence is
`ktlsp-goal-20261010/tenth-java-rename-red-1/artifacts/`, including the actual and
expected snapshots for independent compilation. This strengthens acceptance;
it does not replace the original Java expectations.
The subsequent `tenth-java-rename-red-2` retains the same seventeen checks and
outcome while preserving versioned document changes during URI normalization
and requiring the exact outbound full text for all four changed files.

A separate read-only SNAPSHOT reproduction changes a source JAR at the same
path, without sleeping. Fresh and warm v2 facts and exact goto offsets update,
but displayed text remains v1: **9/12 checks pass, three fail**. Actual javac
accepts both versions, and explicitly rematerializing the source repairs the
display. The lazy marker stores only the JAR path and existing files win over
rematerialization. Evidence is
`ktlsp-snapshot-materialization-20261011/run-1/{results.json,reproduction-report.txt}`.
This is separate from the known second-resolution cache fingerprint limit.
The portable regression adds three dependency and three navigation/dirty-close
cases, all six failing before implementation. One fixes both JAR size and the
whole second while selecting distinct subsecond times with `FileTimes`; it
reproduces the fingerprint collision without sleeps. A warm marker with an
unavailable archive also incorrectly supplies cached facts. Raw failures are
`ktlsp-snapshot-materialization-20261011/implementation/red-{1,2}.out`.

The source-refresh repair now passes all six new regressions, including cold and
warm reloads, legacy markers, replaced/missing archives, removed entries and
closing a dirty materialized source. The related dependency, navigation and
workspace suites pass 69 tests with one ignored. Markers record the captured
archive fingerprint, and reads validate that identity before and after use.
Fingerprints retain full subsecond modification time. Replacing bytes with the
same size and preserved modification time remains an explicit stat-identity
limit; this does not prove automatic Maven SNAPSHOT watching.

The hierarchy repair and source refresh are frozen as `tenth-fixed-2`. All
**1,019 Rust tests pass**, with zero failures and three ignored across 40 suites;
fmt and release compilation also pass. All 270 inputs match before/after tests
and release. The immutable server SHA-256 is
`f42d3c6d60fd62e1f4c86916bc7f331bdbb16896acb812cb6e9721300908d331`.
Evidence is `ktlsp-goal-20261010/tenth-fixed-2/result.json` and the source-refresh
`implementation/final-proof.json`. Editor replays use this binary and retain
their own helper, fixture and source guards.
Before commit, a trailing blank line in the fixture generator is removed.
`tenth-format-2/generator-result.json` proves all four generated files remain
byte-identical. The final `tenth-fixed-3` full test/release run again passes
1,019 tests with all 270 inputs unchanged; both release binaries are byte-identical
to fixed-2, so its editor evidence applies without rerunning unchanged requests.

The new Java editor replay passes **174/179** checks, with exactly the five
previous diagnostic failures remaining. Incoming calls now pass, as do all
seventeen new override rename controls. Neovim applies both declarations and
four Java/Kotlin calls, preserves all five complete buffers and delivers the
four changed full texts before the fresh Kotlin goto request. Actual applied,
independently expected and original snapshots all compile with javac 17 and
Kotlin 1.9.23; all 33 bytecode assertions pass, including the preserved String
overload and unrelated method. Evidence is `tenth2-java-1/artifacts/` and
`tenth2-java-rename-compiler-1/compiler-results.json` under the goal evidence root.
The Java harness still uses forced client stop, so this is not graceful-shutdown
evidence. The known Java bare static call reference omission is retained in
`override-family-safety-first.txt`; its replacement explicit-receiver control
does not count as coverage of that omission. A common-descendant inherited
implementation/interface family safely refuses rename pending a shared identity.

A recount of the unchanged `tenth2-java-1/artifacts/java.out` corrects the previous
177/182 transcription to 174 PASS and 5 FAIL. The expanded bare-call RED run
retains all 179 original check names and adds 17 controls: 181 PASS and 15 FAIL
out of 196, including the same five diagnostic failures. No checks were removed;
the ten additional failures expose the unapplied bare-call rename.

The same release passes **13/13** library source lifecycle checks after an actual
successful Gradle build, and **81/81** JVM facade editor checks. The facade's
baseline and two actual renamed snapshots pass all nine compiler/runtime controls;
the guarded compiler wrapper also verifies that the older baseline fixture and
portable fixture have identical normalized source text. Evidence is
`tenth2-library-1/` and `retrofit-jvm-editor/portable-release-tenth2-1/artifacts/`.
The rebuilt separate external consumer retains **139/140** checks with all five
features and lifecycle controls passing. Its own binary SHA-256 is
`bd53510c198f408c2c6940588fd67025ab7843344d76a0213717063f2a991817`;
the sole failure remains two missing dependency sources. Its 270 product inputs,
consumer, fixtures, probes and binary remain unchanged across build and editor.

CEM and Retrofit retain all **22/22** golden/incomplete first responses across
four guarded runs. Their independent coverage failures remain 1 failed and 5
missing sources each. The subsequent audited Moshi, OkHttp and Spring replays
pass **29/29** semantic requests on attempt 1, giving 51 measured requests across
five real roots. All six additional routed runs still fail independent source
coverage. Gradle golden reaches project indexing but fails library readiness
after an allocation error; it executes zero semantic requests. Gradle incomplete
was interrupted before a result artifact and also remains NOT_MEASURED.
[fork-corpus.md](fork-corpus.md) records the unchanged expectations, corrected
external phase audit and evidence guards. These results establish neither complete
six-root coverage, performance nor current IDEA parity.

### Current slice: Java calls with an implicit receiver

Against `e200d14`, two new focused regressions fail: a bare `assist()` in
`Child extends Base` resolves to Base, Child and an unrelated same-package Decoy;
rename returns three of the four required edits and omits the bare call. This
can silently change dispatch to Base after a compilable partial rename. Raw
evidence is `ktlsp-bare-call-20261011/red-1/` (native cargo exit 101).

The current Java adapter sends implicit member calls through the same JVM
candidate/applicability query as explicit receiver calls. Navigation, references,
rename, signature help and result completion share that selection; signature
assembly no longer re-adds hidden parent methods or unrelated same-name members.
All **61/61** member-selection tests pass, including three new regressions for
exact bare-call references/applied edits and typed overload selection with an
unknown-argument control. Evidence is `green-2/` and `members-green-1/` under that
root. The subsequent release, editor and independent compiler/runtime results are
recorded below. These focused results do not establish broader Java name-lookup
parity.

The unchanged tenth-fixed-2 release also reproduces the omission through the
expanded real Neovim `java` probe in `bare-java-editor-red-1/`: the exact four-site
check rejects its three-site edit, so unsafe edits are not applied. The original
override-rename expectations are preserved. The independent compiler experiment
`ktlsp-bare-call-20261011/compiler-red-1/` captures all twenty expected command
outcomes. Original, independently expected and unchanged actual snapshots compile
and run; the three-site partial-rename counterexample also compiles in Java/Kotlin
but its runtime assertion fails with `1:2:2:1:3` instead of `2:2:2:1:3`. Therefore
compiler validity alone cannot establish safe rename. This RED experiment reports
`passed=false` because Neovim did not apply the required edit; it is not a current
successful editor/rename result.

The first full eleventh checkpoint caught a signature-help regression: only the
two-argument `greet` remained when the existing test also requires the one-argument
overload. That guarded run reports 1,032 PASS, one FAIL and three ignored, native
101; its failed result is preserved in `eleventh-fixed-1/`. Two focused signature
regressions then fail before repair, including explicit/implicit receiver and
unrelated same-name controls. Signature assembly now retains alternative arities
while sharing receiver ownership and argument-type selection; navigation and
rename still use complete-call applicability. The same unchanged expectations
pass **67/67** member/signature tests in `signature-green-1/`, with input guards.

The accepted `eleventh-fixed-2` checkpoint passes **1,034** Rust tests across forty
suites, with zero failures and three ignored. Formatting and both release builds
pass; all 306 product inputs, including 26 classfiles and five module-fixture JARs,
match before tests, after tests and after release compilation. Its immutable ktlsp
SHA-256 is `08fea5789ef522d00b0f4b012febeb555de249c92f8e883df1d268c464d9eab7`.
The real Neovim Java replay passes **191/196** checks: all seventeen new bare-call
controls pass, all five buffers match their independently expected text after the
four-site rename, and delivery/current Kotlin navigation/disk guards pass. The
same five diagnostic failures remain; native harness exit is one. Evidence is
`eleventh2-java-1/{java-verified.json,artifacts/}`. The independent
`ktlsp-bare-call-20261011/compiler-green-1/compiler-results.json` verifies all twenty
expected native command outcomes. It derives the four edits and expected five
buffers independently, compiles the actual applied Java/Kotlin text and observes
`2:2:2:1:3` at runtime. The three-edit counterexample still compiles but fails the
runtime assertion with `1:2:2:1:3`; all input guards match. This is a passing applied
rename slice, not an all-green Java acceptance result.

### Current slice: bounded watch registration and algorithm review

`project_watchers` now coalesces internal source/output directories under one
workspace pattern. External source/output directories and exact local binary and
source JAR paths retain their existing boundaries. Models without internal
source/output roots keep the two configuration/archive patterns. Event relevance
still follows the published model, including arbitrary output extensions and
unrelated-source exclusions.

The regression uses the observed 4,869 source and 3,925 output directories. Before
repair it returns 8,796 patterns instead of one; afterward both absolute and
relative registrations have one pattern and serialize below 1 KiB. Six relevant
tests pass, including create/change/delete, external future-directory escaping,
empty/external-only models and existing refresh coalescing. Evidence is
`ktlsp-watch-registration-20261011/final-proof.json`. The guarded full/release
checkpoint above includes this repair. The same large Gradle workload through
real Neovim now registers one model pattern, acknowledged in the same log second.
Neovim lifetime peak commit drops from 63,475,847,168 to 9,261,056 bytes; server peak
commit remains 5,969,633,280 bytes. All three golden requests match on their first
response. The module-only repair reduces missing sources from thirty to twenty-five;
three failed inputs remain. This is one diagnostic comparison, not statistical
performance acceptance or a steady-state measurement. See [fork-corpus.md](fork-corpus.md)
for process/system measurements and unchanged-work evidence. The controller and
server both exit one: dependency coverage fails, and the raw shutdown exchange has
a duplicate request and an Invalid request response. No resource cap fired and no
process termination API was called. Complete editor lifecycle verification remains
open.

The broader review found three concrete scaling paths that require measurement
and follow-up: notification classification scans model roots per event and
rebuilds local-JAR sets; inference snapshot refresh clones search maps after an
index revision; ordinary completion queries candidates twice before returning the
response. Shared symbol payloads already use `Arc`, so this is not evidence of a
complete deep copy of every symbol. These findings do not establish the cause of
the server's measured peak or supply accepted performance results. Optimize the
shared paths with unchanged semantic expectations, then compare useful editor
responses and separate client/server memory on the same real roots.

### Current slice: completion work, cache copies and observed shutdown

The ordinary LSP completion path now obtains trace metadata and rendered items
from one semantic query. Public `Workspace::complete` keeps its provider-first
behavior; the LSP wrapper keeps the previous query-before-provider ordering.
Candidate shaping, rank, snippets, limits and honest unknown results are unchanged.
Focused library, extension-host and member-selection tests pass 299 checks with
one ignored. Global prefix scanning and inference lookup-map copying remain open.
A gated `inference_snapshot` span measures replacement time in the actual Salsa
sync path; it is not an allocation or retained-memory measurement.

Gradle cache persistence now serializes a borrowed view through `BufWriter` and
decodes through `BufReader`. It avoids cloning the complete module model and
holding the complete JSON string, while preserving schema version four. Each
writer owns a unique `create_new` temporary file; errors retain the previous
cache. All 24 classpath tests pass, including partial disk writes, simultaneous
writers, exact wire compatibility, mapped sources/compiler options, invalid-cache
fallback and unpublished loading. The original implementation fails the two new
failure/concurrency controls. The first repaired run exposed stale Windows
directory-entry size observations; fresh file reads retain the real partial-write
assertion and pass. Evidence: `ktlsp-classpath-persistence-20261011/implementation-1/`.
The whole-release large-cache comparison below establishes no measured memory
or speed improvement; it does not isolate this persistence change.

The mixed, corpus, UI and live Gradle probes now observe the shutdown response and
actual `on_exit` result. They require one acknowledged shutdown, one exit
notification, native exit/signal zero and no forced termination. Waiting for
`client:is_stopped()` previously returned while the client was still stopping;
Neovim's exit then sent a duplicate shutdown. Four direct protocol controls
confirmed normal server exit zero independently. Fourteen portable client controls
pass, including timeout, error, duplicate, callback preservation and idempotence.
These controls do not replace a real-editor replay with the new helper. The first
mixed replay exposed a caller logging error: the mixed/corpus printers expected
a string rather than the structured shutdown result. Both call sites now render
that detail explicitly; the structured result remains in the report. The failed
setup run is preserved with zero UI samples and is excluded from timing results.

The guarded `twelfth-fixed-2` checkpoint passes **1,037** Rust tests across forty
suites, zero failures and three ignored tests, using Rust 1.98.1. Formatting and
both release builds exit zero; all **307** product inputs match before tests,
after tests and after the release build. The immutable ktlsp binary SHA-256 is
`7c17b5f0991774b37c7cee3bf73936de426f2476f51ada8e3e984a970670dce0`;
bench is `6352515f7e6dcdb62b8f87e892f262dd8b5010c0bd674a2dd982ecb8226693d0`.
Evidence: `ktlsp-goal-20261010/twelfth-fixed-2/`. The preceding fixed-1 binary has
the same SHA; only the two Lua call sites changed. The new helper still passes
all fourteen controls and six Lua parse checks. Both immutable releases then ran
through the same fixed-2 Neovim harness. The mixed fixture supplies one first and
thirty useful warm-process observations for each language; all 62 exact accepted
texts pass per release. Every repetition makes unsaved edits, rather than querying
one unchanged index revision. Times below are milliseconds; popup timing ends at
the `popupmenu_show` handoff, not physical display.

| Release | Language | First RPC | Warm RPC p50 / p95 | Warm dot-to-menu p50 / p95 |
| --- | --- | ---: | ---: | ---: |
| Eleventh-fixed-2 | Java | 2.359 | 2.394 / 2.952 | 33.848 / 34.550 |
| Eleventh-fixed-2 | Kotlin | 2.714 | 2.473 / 3.179 | 33.860 / 35.180 |
| Twelfth-fixed-2 | Java | 2.293 | 2.539 / 3.987 | 33.933 / 36.141 |
| Twelfth-fixed-2 | Kotlin | 2.146 | 2.525 / 2.969 | 33.910 / 34.743 |

This single matched pair shows no established speedup. Brief read-only preparation
overlapped one mixed run, and whole-release timing cannot isolate completion work
from cache/tracing changes. Two baseline and four candidate trace lines are
malformed; client-observed timing/accepted-text records remain intact. No valid
inference-snapshot spans were observed, so snapshot cost is **not measured**.

CEM's ordinary four completion/navigation assertions pass on both releases, with
the same 34,783 library files and 825,029 symbols. Coverage still fails with one
failed and two missing source inputs. The new UI case replaces an unsaved line
with `return super.`: both releases return an empty first completion with
`unknown-receiver-type`, so neither has a useful UI sample or warm timing result.
The class has a superclass and a delegated interface; the multi-super receiver
selection is the next focused reproduction, not a confirmed fix. Empty replies
are excluded from performance acceptance. Source, binary and fixture guards match
throughout all four runs. Mixed and CEM both record acknowledged shutdown, one
exit notification and actual exit/signal zero without forced termination in their
RPC and UI phases, independently of semantic/coverage failures. Evidence is
`ktlsp-completion-measurement-20261011/run-{baseline,candidate}-2-{mixed,cem}/`.
The aggregate four-run report is `ktlsp-completion-measurement-20261011/four-run-report.json`
(SHA-256 `0329435bff9473496ec138fc9f40fe74c03569525acfa11bbef375f308b57ce5`).

A separate consumer rebuilt against the frozen 307-input twelfth snapshot passes
**140/140** unchanged real-Neovim assertions: twenty-nine first responses,
nineteen delivered-edit barriers and four applied/fully inspected renames.
Dependency indexing reports zero failed and missing inputs. Consumer build,
editor and independent finalizer exit zero; shutdown records one null
acknowledgement, one exit notification, actual server exit/signal zero and no
termination or remaining owned processes. Product, four consumer inputs,
twenty-seven fixture sources and both probe files match throughout. Its distinct
consumer binary SHA-256 is
`a059b133c5a3a89c3d3a4fe088aae39043f573b08b962f8cc5940002cfefcb98`.
Evidence: `ktlsp-shadow-twelfth-replay-20261011/final-editor-proof.json`
(SHA-256 `d30011d306fd69924c4fa906044484a07720ddea670a0443232cf2357986a850`).
The eleventh replay's duplicate shutdown remains a separate historical limitation.
The server peak near 6 GB and compiler-plugin gaps remain unresolved; this
checkpoint does not establish whole-project performance or IDEA parity.

### Warm Gradle cache: matched server memory baseline

Eleventh-fixed-2 (A) and twelfth-fixed-2 (B) ran in ABBA order against the same
dedicated Gradle cache, with no concurrent compiler, editor or build workloads.
All four runs preserve the same 932 cache files by content, 293 source archives,
selected project/model inputs and immutable release binaries. The model may be
resaved with different filesystem timestamps while retaining identical bytes.
Each run records ten seconds of post-index RSS (101 samples); the table uses
that interval's median. Decimal MB are used for memory, seconds for time.

| Run | Project index | Library index | Steady RSS MB | Lifetime peak commit MB | Shutdown |
| --- | ---: | ---: | ---: | ---: | ---: |
| A1 | 6.703 | 9.453 | 3,704.762 | 3,928.834 | 10.391 |
| B1 | 7.328 | 10.156 | 3,705.127 | 3,930.870 | 10.578 |
| B2 | 6.890 | 9.562 | 3,705.537 | 3,932.115 | 10.750 |
| A2 | 6.500 | 9.266 | 3,699.962 | 3,926.401 | 9.688 |

Both releases retain approximately 3.7 GB after indexing and peak at 3.93 GB
commit on this warm workload. Two runs per arm do not establish a speedup or a
p95, and whole-release comparison cannot isolate `BufReader` or query work.
CPU is not measured. All four servers acknowledge shutdown once and exit zero
without termination; their roughly ten-second cleanup remains a latency gap.
The benchmark's former three-second wait forced termination after a valid
shutdown acknowledgement in the unmeasured cache warmup. That failed warmup is
retained. The client now waits a bounded fifteen seconds, records actual shutdown
duration, and still rejects missing acknowledgements, nonzero exits and forced
termination. Its nine controls pass, including the original timeout/kill checks.
Increasing the measurement wait does not make slow shutdown acceptable UX.

Each measured run indexes the same 60,746 library files and 1,245,317 symbols,
including 13,891 JDK files and 285,542 JDK symbols, from 320 dependencies.
The server-only runs report 11,327 project files; each editor run reports 11,325
closed project files because its two open buffers are indexed separately.
Three failed and twenty-six missing source inputs keep coverage **FAIL**;
`steady_state_accepted` remains false. The existing real-Neovim corpus route then
passes all three exact first semantic checks on each release, without retries.
Those correctness-only runs also retain coverage failure and clean native exits.
The first navigation normally materializes one JDK `Collection.java` file whose
bytes match the guarded source ZIP entry. Its original cache guard failure is
preserved; the second correctness run starts from that 933-file state. These
editor runs are not a matched timing extension of the ABBA experiment.

Evidence: `ktlsp-server-memory-pair-20261011/{ABBA-statistics.json,ABBA-times.json,
A1.json,B1.json,B2.json,A2.json,corpus-A-cache-delta.json,guards-corpus-B.json}`
and `shutdown-client-checks.json`. Existing watcher repair prevents the separate
63 GB client allocation failure; it does not solve this server footprint.

That baseline exposed a separate allocation problem:
bincode preserves type-context values but reconstructs a separate candidate
`Arc` per `TypeRef`. Three immutable cache copies (Guava, Gradle profiler and
Kotlin stdlib) contain 67,573 duplicate candidate allocations within files.
The stdlib sample has 25,756 candidate pointers but only 591 distinct ordered
candidate lists summed per file. These are archive populations, not final selected
index retention or RSS. Native Rust 1.98.1 layout measurements find
`IndexedSymbol` at 696 bytes and `TypeRef` at 112 bytes; slot sizes alone do not
prove allocation savings. Evidence: `ktlsp-warm-sharing-20261011/baseline-receipt.json`.
The following checkpoint restores identical ordered candidate-list sharing
after accepted cache loads. Inline optional type slots and decode-buffer peaks
remain separate work.

### Thirteenth checkpoint: cache sharing and bounded notification lookup

Accepted library caches now restore file-local sharing of identical, ordered
type-context candidate lists. The same core helper handles nested arguments,
all indexed type fields and JVM projection facts in both the LSP per-archive
loader and ktcheck's aggregate loader. Explicit-import lists, order, duplicates,
qualifiers, nullable types and container contexts retain their original values.
The pool is dropped after each file; no global interner or new dependency is
introduced. Cache versions remain unchanged because full serialized bytes are
identical. Actual old-loader regressions fail before the fix and pass afterward.
Focused checks pass: core 1, dependencies 38 and CLI 13.

On the same immutable Guava, profiler and Kotlin stdlib cache copies, the actual
helper eliminates all 67,573 duplicate candidate allocations counted within
files. The stdlib candidate vector/string payload capacity falls from 9,362,886
to 123,845 bytes. This excludes Arc headers, allocator metadata and other data;
it is an archive population, not the authoritative selected index or process
memory. Restoration happens after decoding and therefore cannot eliminate the
earlier deserialization allocation peak. Evidence is
`ktlsp-warm-sharing-20261011/{baseline-receipt.json,candidate-receipt.json}`.

Project ownership lookup also stops scanning every registered root for each
file or notification. A shared ancestor walk probes ordered maps by path depth,
preserving longest-root, source-over-module, first-owner, canonical-alias and
exact-artifact rules. Dependency keys are prepared once per project snapshot;
notification reduction stops after the first forced refresh. It does no new
per-event filesystem or Gradle work. Focused classpath 29 and LSP 26 checks pass.
Canonical-host UNC file URIs remain a separate known gap: the existing vendored
URI conversion loses their authority before ownership classification. Native
UNC boundary tests do not establish support for that URI form.

Thirteenth-fixed-1 passes formatting and all 1,049 workspace tests, with three
ignored tests, then builds both release binaries. All 307 compiled/harness inputs
are identical before tests and after release. Server SHA-256 is
`18b0a822071e85fa2cfd25512b1305c331e53b788de3ae57edc4bf4231584095`.
The checkpoint receipt is `ktlsp-goal-20261010/thirteenth-fixed-1/result.json`.

A new exclusive ABBA comparison uses twelfth-fixed-2 (A) and thirteenth-fixed-1
(B), the same existing 933-file warm cache, 293 source archives and guarded
Gradle model. There is no new warmup, cache copy or concurrent heavy workload.
Each RSS observation again covers ten seconds after indexing, with 101 samples.

| Run | Project index s | Library index s | Steady RSS MB | Lifetime peak commit MB | Shutdown s |
| --- | ---: | ---: | ---: | ---: | ---: |
| A1 | 6.812 | 9.547 | 3,703.988 | 3,927.740 | 9.500 |
| B1 | 5.500 | 8.125 | 3,431.354 | 3,648.434 | 8.063 |
| B2 | 5.281 | 7.922 | 3,427.787 | 3,646.968 | 8.109 |
| A2 | 6.000 | 8.593 | 3,701.977 | 3,927.249 | 9.625 |

Median steady RSS is 3,702,982,656 bytes for A and 3,429,570,560 for B: an
observed reduction of 273,412,096 bytes (7.38%). Library readiness medians are
9.070 and 8.0235 seconds, and shutdown medians are 9.5625 and 8.086 seconds.
This is a whole-candidate comparison: sharing and owner lookup changed together.
Two runs per arm, uncontrolled OS page cache and unmeasured CPU do not establish
a general speedup, p95, isolated cause or accepted performance budget. Shutdown
still takes seconds. RSS and lifetime commit counters describe different scopes.

All runs retain exactly 60,746 library files and 1,245,317 symbols, with the same
11,327 closed project files in server-only runs. Three failed and twenty-six
missing source inputs keep coverage and steady-state acceptance **FAIL**.
Correctness-only Neovim companions pass all six exact first semantic requests
without retries; their two open buffers explain the 11,325 closed-file count.
All six servers acknowledge shutdown once, receive one exit and return native
zero without forced termination. All cache, source, archive and model guards
pass, and all verified owned processes exit. Evidence is
`ktlsp-warm-sharing-pair-20261011/{final-receipt.json,ABBA-statistics.json}`.
The existing Neovim library replay passes 13/13. Same-client Gradle refresh
replays pass 70/72 with explicit standard watched-file notifications and 73/76
with Neovim's automatic watcher. All eight offline builds exit zero. Dependency
addition, version update, removal, standard/custom generated roots and preserved
unsaved cross-language members pass without changing their expectations.
Both modes still miss the two expected stale-member diagnostics after updating
the dependency. Each mode also has two query points where the first response
is empty and a later polled response becomes useful; eventual success is not
first-response freshness.

The explicit replay observes one shutdown acknowledgement, one exit and native
zero without termination. The automatic replay receives the acknowledgement
and sends exit, but does not observe the native exit within its existing ten
seconds; its timeout/force request remains a third failure. No terminate call
is recorded, and the process is gone in the final owned-process inventory.
The cause of this close failure is not established: these are correctness runs
with concurrent compiler/IDEA work, not matched shutdown performance samples.
The old library route does not observe protocol shutdown, so its wrapper's zero
exit is not a server clean-exit proof. All 307 source inputs, binaries and
controllers remain unchanged. Evidence is
`ktlsp-semantic-gaps-20261010/thirteenth-replay-preparation-1/{final-receipt.json,
first-responses.json}` and the three `thirteenth-*` Gradle harness directories.
IDEA parity, compiler-producer freshness and full source coverage remain open.

### Fourteenth checkpoint: specialized receiver storage

The three immutable archive samples contain 28,071 symbols in 1,146 files.
Only 44 symbols have a trailing-lambda receiver type, all in the Kotlin stdlib;
none has a receiver-function typealias slot. These sample counts do not establish
occupancy in the final selected live index. They justify first moving just these
two specialized optional fields out of line, retaining ordinary return/value and
parameter type storage and the existing inference and summary APIs.

Both fields now use `Option<Box<TypeRef>>`. On the actual Windows Rust 1.98.1
release, `IndexedSymbol` measures 488 bytes instead of 696; each optional boxed
slot is 8 bytes instead of the 112-byte inline optional type. A present receiver
requires its own allocation, so the object-size reduction is not a process-memory
measurement. The four producers/readers and cache-sharing traversal are updated.
Direct external writers of these public indexed fields must wrap their values in
`Box::new`; `TypeRef` itself and provider-visible member signatures are unchanged.

An explicit previous-layout fixture covers empty and rich populated symbols,
including generics, nullable and qualified types, ordered/duplicate candidates
and both receiver slots. It proves identical JSON and bincode bytes and decoding
in both directions. The new native counter also reads and reserializes all three
actual previous-release cache copies byte-for-byte, preserving their counts.
Cache schema versions remain unchanged. Evidence is
`ktlsp-receiver-slot-occupancy-20261011/receipt.json` and
`ktlsp-receiver-slot-fourteenth-20261011/receipt.json`.

Fourteenth-fixed-1 passes formatting, all 1,050 workspace tests with three
ignored, and both release builds. All 307 compiled/harness inputs remain unchanged
through verification. Server SHA-256 is
`f9c780b86d6506af2c94046eeb7b98e81ac6b1f7fd3a7f7908c6e1dc106d5611`.
The checkpoint receipt is `ktlsp-goal-20261010/fourteenth-fixed-1/result.json`.
The separate extension consumer builds offline and passes all 140 editor
assertions, including 29 first-response requests, 19 notification barriers and
four applied and inspected renames. It shuts down with one null acknowledgement,
one exit notification and native exit 0, without forced termination. Product,
consumer and fixture guards remain unchanged. Evidence is
`ktlsp-shadow-fourteenth-replay-20261011/final-editor-proof.json`.

The unchanged semantic harness passes all 18 assertions, covering alias-backed
implicit receivers, scope/result chains and KMP narrowing. Its existing
`client:stop(true)` finalizer yields server code 1/signal 15 despite wrapper exit
0; clean shutdown is therefore unproved on this route. Both replays leave no
owned processes. Evidence is
`ktlsp-semantic-fourteenth-replay-20261011/semantic-final-receipt.json`.
An additive finalizer-only control uses the same accepted fourteenth binary,
unchanged semantic assertions and the existing shared lifecycle helper instead
of `client:stop(true)`. It passes 19/19, including one null shutdown acknowledgement,
one exit notification and native exit 0/signal 0 with no termination or timeout.
All copied inputs remain unchanged. The helper's existing 14 mock lifecycle
controls also pass, and seven Lua files parse. The harness now saves
`semantic-shutdown.json`; the original forced-exit evidence above is retained.
This control proves the finalizer, not fifteenth behavior or shutdown speed.
Evidence is `ktlsp-semantic-shutdown-control-20261011/receipt.json` and
`ktlsp-shutdown-stage-profile-20261011/lifecycle-selfcheck-after-semantic-finalizer.json`.

The exclusive matched warm-cache ABBA comparison uses unchanged thirteenth-fixed-1
(A) and fourteenth-fixed-1 (B), the same 933 cache files, 293 source archives,
55,170,313-byte Gradle model and 307 product/harness inputs. No cold run, fresh
warmup or cache copy is introduced. Observed values are:

| Run | Steady RSS bytes | Lifetime peak commit bytes | Library readiness ms | Shutdown ms / outcome | RSS samples |
| --- | ---: | ---: | ---: | --- | ---: |
| A1 | 3,429,761,024 | 3,649,335,296 | 8,922 | 7,843 / native 0 | 101 |
| B1 | 2,851,864,576 | 3,049,566,208 | 8,438 | 6,031 / native 0 | 101 |
| B2 | 2,850,316,288 | 3,047,612,416 | 8,156 | 6,047 / native 0 | 101 |
| A2 | 3,418,877,952 | 3,637,071,872 | 8,297 | 15,297 / forced timeout, native 1 | 100 |

The descriptive RSS medians are 3,424,319,488 and 2,851,090,432 bytes: an observed
reduction of 573,229,056 bytes (16.7399%). Each window is ten seconds. A2 misses
the expected 101st sample and fails the unchanged 15-second normal-exit wait
after a null shutdown acknowledgement and exit notification; no retry or timeout
change hides either failure. Its shutdown duration cannot estimate successful
shutdown speed. CPU is unmeasured and n=2 per arm cannot establish p95 or general
latency acceptance.

The two existing-corpus companions pass all six exact semantic requests on their
first attempts and both shut down normally. Five of six server runs exit native
0; all verified owned processes are gone and every binary, helper, cache, model
and source guard passes. All runs retain three failed dependencies and 26 missing
sources, so complete indexing, accepted steady state, performance and IDEA parity
remain false. Evidence is
`ktlsp-receiver-storage-pair-20261011/{final-receipt.json,ABBA-statistics.json}`.
This storage change does not close the existing Gradle diagnostics, producer
freshness, source-coverage or CEM `super.` completion gaps.

An additional instrumented lifecycle sample links against the accepted fourteenth
release libraries, preserving the benchmark's shutdown routine and deadline.
It receives a null acknowledgement, sends exit and returns native 0 without
termination in 6,359 ms. During that interval it consumes 6,296.875 ms of CPU
(453.125 kernel and 5,843.75 user); private memory falls from 2,985,484,288 bytes
to 204,394,496 bytes at the last live observation. Stderr stage markers place
runtime destruction after transport return and measure it at only 1.9043 ms.
This sample supports CPU work and substantial data release before transport
returns, rather than an idle runtime waiting for stdin. It does not isolate a
particular container's destructor or establish the cause of the earlier A2
timeout. All captured compile/monitor input guards pass, all four owned processes
exit, and all three retained monitor handles close. This is one diagnostic sample,
not a replacement timing baseline or performance acceptance. Evidence is
`ktlsp-shutdown-stage-profile-20261011/diagnostic-summary.json`.

### CEM blank-super oracle: inspect the accepted code

Expected behavior is a legal direct-super call after accepting completion in
`EnchantableDrillBlockEntity.getBreakSpeed`. ktlsp currently returns no useful
blank-`super.` completion. The native IDEA LSP 263.6379.0 popup returns 172 items
on its first request, including `breakSpeed (from getBreakSpeed())`. Its empty
text edit defers insertion to `jetbrains.kotlin.completion.apply`; the label alone
does not establish what accepting the item does.

An additive native-Neovim acceptance probe selects that actual item, captures
completion resolve, the deferred command and `workspace/applyEdit`, and retains
the complete resulting buffer. IDEA inserts `return super.breakSpeed`, failing
the compiler-legal method-call expectation. The original strict method-label
failure is retained. The user's source files are unchanged.

The captured full buffer is compiled verbatim with CEM's selected 101 classpath
binaries, 156 guarded project output files, Kotlin 2.4.0, JDK/target 21 and the
actual task's scripting plugin paths. It fails with the sole diagnostic
`unresolved reference 'breakSpeed'` at line 34. A separate control replacing only
that line with `return super.getBreakSpeed()` compiles; javap proves an
`invokespecial DrillBlockEntity.getBreakSpeed:()F` call. All 1,061 guarded inputs
remain unchanged. Evidence is
`ktlsp-cem-blank-super-20261011/{deferred-final-proof.json,
real-cem-accepted-results.json}`.

This is a failure for this CEM context, not a general prohibition on Java getter
properties through `super`: separate public/protected Java controls compile and
run successfully. No game launch or producer-closure claim follows. The next
shared-layer fix must select legal members per direct-super candidate, preserve
override/hiding and generic substitution, and distinguish abstract, static and
inaccessible members before treating multiple providers as ambiguous.

### Sixteenth checkpoint: shared effective direct-super members

The shared member selector now evaluates each exact name across the instantiated
direct-super receivers. It hides overridden ancestors before checking concrete
implementation and source access. Distinct concrete names remain useful in a
blank popup; an abstract provider does not compete with a concrete implementation,
and a reabstracted override cannot resurrect its concrete ancestor. Unknown or
competing providers remain inconclusive for their affected names.

One normalized byte records source member implementation, dispatch and access.
It is produced by both language indexers and reused by source facts and Kotlin
super selection, independently of JVM descriptor certification. Constructors
remain distinct from ordinary methods. Concrete properties and Java fields retain
type inference, definitions and result chains, including Kotlin init-block,
lateinit and delegated initialization. Native/external source calls do not certify
unsupported JVM identities; internal access still needs module/friend evidence.
The actual Kotlin 2.4 compiler and runtime establish package access through a
foreign intermediate superclass; javac's different result is retained separately.

The byte fits the existing 488-byte native symbol layout without another heap
allocation. It appends a 22nd serialized field, so the dependency and CLI cache
schemas advance to v21 and v11. The previous 21 field values are retained, but full
old/new wire equality is not claimed. Property regression REDs against the exact
fourteenth binary and independent compiler controls are retained in
`ktlsp-cem-blank-super-20261011/source-hold-packet.json`.

Navigation, return inference, trailing-lambda inference and both call-diagnostic
readers now retain the same filtered effective pool. Recollecting ordinary members
from only its receiver had reintroduced private vararg overloads, yielding two
goto targets and an Unknown return type. The focused runtime RED, 180 passing
related tests and independent Kotlin 2.4 compile/runtime controls are retained in
`ktlsp-cem-fifteenth-fixed-1-20261011/shared-super-pool-final/report.json`.
The existing editor scenario adds first-response exact goto and return-chain
completion controls; closed-file goto also rejects a unique private-name fallback.

Sixteenth-fixed-1 passes formatting, all 1,076 workspace tests with three ignored,
and both release builds. All 307 inputs remain unchanged. Server SHA-256 is
`51e2b74adee7797f5a65a727c6e8152861b4de181dede46668b355e94646c11d`.
The accepted release passes the semantic editor route 23/23, with one null
shutdown acknowledgement, one exit notification and native exit 0/signal 0,
without termination. Its roughly 4.2-second shutdown still exceeds the two-second
target. Evidence is `ktlsp-goal-20261010/sixteenth-fixed-1/result.json` and
`ktlsp-semantic-sixteenth-replay-20261011/receipt.json`.

The separate extension consumer builds offline with unchanged Rust/lock inputs
and passes all 140 editor assertions. All 29 requests succeed on their first
attempts, with 19 delivery barriers and four applied and inspected whole-buffer
renames. One null acknowledgement, one exit notification and native exit 0/signal
0 prove clean shutdown without termination; no owned processes remain. All
307 product, four consumer and 27 fixture source guards remain unchanged.
Evidence is `ktlsp-shadow-sixteenth-replay-20261011/final-editor-proof.json`.

The fifteenth real-CEM failure remains preserved: both getBreakSpeed requests and
native popup acceptance failed, so no accepted-buffer compiler check was run.
Its hierarchy included Ponder's VirtualBlockEntity, whose cached sources were
never admitted. The shared coordinate parser rejected literal `+` characters,
silently dropping Ponder, Registrate, Curios and sponge-mixin. A one-character
parser repair preserves their selected versions and source paths while retaining
path/query/traversal rejection. Actual-coordinate parser and selected-model
regressions prove the failure and repair. No incomplete-hierarchy gate was relaxed.

The fresh sixteenth CEM replay seeds only the unchanged selected model, without
old source/symbol caches. It indexes 35,672 library files with 838,391 symbols from
101 dependencies, alongside the existing 5,865 local-jar source files. All four
strict first-response completion/goto assertions pass. The one native popup
sample accepts `return super.getBreakSpeed()`; its complete 85-line buffer is
checked against the single manifest line replacement and captured verbatim.
That exact buffer compiles with Kotlin 2.4/JDK 21, 101 classpath entries, 156
project outputs and the seven selected scripting plugin-path JARs. Javap proves
one invokespecial of DrillBlockEntity.getBreakSpeed:()F. This task configures no
KAPT/KSP/Lombok/serialization transform and supplies no producer-freshness proof.

Both CEM clients acknowledge shutdown and exit normally without termination.
All 1,387 editor guards, the current 395-file inventory and 12 explicit absent-file
guards remain unchanged; the compiler's 1,389 guards also match. Those absent
files were empty game-world/download data in the older temporary copy; its
original 407-file comparison failure is preserved. The strict harness still
exits 1 because commons-compress 1.26.0 fails UTF-8 decoding and patchy 2.2.10 plus
kotlinforforge-neoforge 5.12.0 lack sources. Evidence is
`ktlsp-cem-sixteenth-fixed-1-20261011/{run-cem-first,
actual-accepted-compiler}/result.json`. Next address shared snapshot allocation
and declared source encodings; compiler-producer admission, unknown ancestor
overloads, classifier/source visibility and six-project IDEA parity remain open.

### Seventeenth checkpoint: share inference snapshot buckets

The three private inference lookup maps now share immutable string keys and
entry buckets with their snapshots. Existing-key insertion allocates no key;
editing detaches only the affected buckets. The public borrowed lookup API,
selection order, source facts and serialized caches are unchanged. The existing
snapshot regression first fails at runtime (symbol reference count 8 instead of
4); after the repair, all 38 focused index/Salsa/workspace checks pass. They cover
same-FQN library survivors, frozen ownership/type/JVM revisions, replacement,
removal and unchanged buckets as well as closed and open Kotlin query paths.

Seventeenth-fixed-1 passes formatting, all 1,076 workspace tests with three
ignored and both release builds. All 307 input guards match. Its server SHA-256
is `7f9037c7819395f975075a46dae2b0a007bc51d69a126f8ae1f1fe153a83b8f8`.
The release semantic editor passes 23/23 with one acknowledged shutdown, one
exit notification and native exit 0/signal 0, without termination. The roughly
4.4-second shutdown still fails the two-second budget. CEM and the external
consumer evidence above remain sixteenth results; they were not rerun for this
private storage change. Evidence is `ktlsp-goal-20261010/seventeenth-fixed-1/`,
`ktlsp-semantic-seventeenth-replay-20261011/` and the measurement directory below.

The before/after release helpers parse the same three guarded source JARs into
28,071 symbols and 10,740 lookup buckets. No old bincode cache is decoded. Each
arm records 30 raw samples, uses the identical allocator self-check and helper
source, and verifies unchanged inputs/artifacts plus identical symbol inventory.

| Isolated measurement | Sixteenth | Seventeenth |
| --- | ---: | ---: |
| Snapshot requested allocation bytes | 2,916,559 | 518,448 |
| Snapshot allocation operations | 21,483 | 3 |
| Snapshot p50 / p95 (ms) | 2.0232 / 3.9297 | 0.0771 / 0.1601 |
| Unique base-index retained bytes | 77,767,080 | 77,908,625 |
| Unique base-index retained allocations | 311,574 | 322,314 |

The snapshot allocation falls 82.2%, at a base cost of 141,545 requested bytes
and 10,740 allocations. Snapshots still copy three hash tables in O(bucket count);
the first mutation of a shared bucket copies that bucket's entries. This is
allocation-instrumented isolated indexing, excluding allocator overhead, C
allocations, RSS and snapshot destruction time. It is not LSP timing or a new
large-project memory result. Production snapshots occur when Salsa synchronizes
after an index revision, rather than on every request. Raw proofs are
`ktlsp-index-snapshot-cow-measurement-20261011/{sixteenth-before-1,
seventeenth-after-1}/receipt.json`; implementation and runtime RED evidence are
in `implementation-1/source-stable-receipt.json` in that directory.

### Eighteenth checkpoint: declared source encoding and real CEM regression

Java source archives now use an exact UTF-8 or ISO-8859-1 decoder selected from
cached published POM declarations and their static parent/property chain. This
does not run Maven or establish an effective compiler/plugin model. XML and
metadata work have explicit size/depth limits; archive CRC errors, unsupported
encoding declarations and uncertain conditional declarations remain errors.
Kotlin and JDK source stay strict UTF-8. Policy receipts include exact GAVs and
POM content hashes in symbol-cache and lazy-materialization identities. Published
POM fixtures preserve their raw bytes through a narrow Git attribute.

The integrated focused suite passes 74 checks with one existing download case
ignored. Eighteenth-fixed-1 passes formatting, 1,091 workspace tests with three
ignored and both release builds, with all 312 input guards unchanged. Server
SHA-256 is `abe40648e15965e56762e3b43a793d96bf1e3d8c30d8f16d00575cfa86f27b41`.
The actual library editor replay passes 13 source/JDK lifecycle assertions,
including unsaved rename/delete and closed-buffer restoration. That existing
scenario does not capture a normal shutdown acknowledgement; it is unmeasured.

The fresh CEM replay retains the original four assertions and adds three first
responses for incomplete/complete `TarArchiveOutputStream.getBytesWritten()`.
All seven pass after actual unsaved delivery. The selected 1.26.0 source is
materialized losslessly as UTF-8, with exact whole-file hash and UTF-16 range
433:16-31. Independent Java/Kotlin clients compile against the actual selected
binary and each emit the expected `getBytesWritten:()J` call. Native popup
acceptance again captures the entire 85-line Kotlin buffer verbatim; compilation
and the exact `DrillBlockEntity.getBreakSpeed:()F` invocation both pass.

All 17 CEM semantic/UI/guard conditions pass, with acknowledged shutdown and
native exit zero for both clients. Strict corpus exit remains **1**: missing
Patchy and KotlinForForge sources remain, and Guava 32.1.2-jre plus Commons Codec
1.16.0 now fail metadata admission. Guava has an interpolated JVM boot-classpath
argument; Codec's parent repeats unrelated release-manager properties. Neither
failure is hidden by the seven useful responses. Library coverage is 35,321
files / 827,622 symbols, with two failed and two missing inputs. Next resolve
only the selected encoding declaration closure, preserving ambiguous encodings
and argument-boundary controls. Policy arrival watching, existing-file fast
paths and concurrent materialization remain separate limitations.

Evidence: `ktlsp-source-encoding-20261011/live-integration-1/`,
`ktlsp-goal-20261010/eighteenth-fixed-1/`,
`ktlsp-library-eighteenth-replay-20261011/`,
`ktlsp-cem-compress-compiler-20261011/native-1/result.json` and
`ktlsp-cem-eighteenth-fixed-1-20261011/`. This is not six-root IDEA, latency,
generated-API or game-runtime acceptance.

Compiled-member admission remains unimplemented in the normal server. A separate
Temp observer ran nine actual Gradle invocations: four successful selected-Java
task results have an independently checked Lombok `getName(): String`; five
expected failures remain inadmissible, including failure after valid classfiles
were emitted, disabled processing and an ancestor `lombok.config` change.
Kotlin `String` to `Int` changes are captured in Java's ordered classpath after
the upstream task finishes. All owned processes exited and fixture guards match.
This proves the fixture's observed read set, not arbitrary processor input
closure or unsaved generated-API support. The original twelve completion failures
are unchanged. Evidence is
`ktlsp-compiler-java-receipt-proof-20261011/evidence-summary.json`.

### Nineteenth checkpoint: selected encoding declaration closure

The two CEM metadata regressions above were reproduced independently against
the actual published POM chains. The reader now rejects differing duplicate
properties only when the selected encoding depends on them; child declarations
replace the corresponding parent property. Identical duplicates are unambiguous.
A fixed `-J-Xbootclasspath/p:` compiler-argument leaf stays one JVM argument even
when its path payload is interpolated. Argument files, dynamic option selectors,
unknown argument transport/merge shapes and ambiguous selected encodings still
fail. This is a bounded declaration reader, not a general effective Maven model.

The original nine focused tests remain unchanged. Three new tests first fail
against the old reader (native exit 101), then all twelve pass (native exit 0),
including fourteen argument-boundary rejection controls. Actual Guava and Codec
source archives also pass CRC and strict UTF-8 decoding for 624 and 80 Java
entries respectively; decoding alone does not prove editor semantics.

Nineteenth-fixed-1 passes formatting, 1,094 workspace tests with three ignored
across 40 suites, and both release builds. All 312 inputs match before/after
tests and release. Server SHA-256 is
`ce472b72a170632dc70346d7aa83c667e3134ac48f83b041c20592334cd93652`.
The fresh CEM editor replay passes all seventeen semantic/UI/guard conditions,
including all seven original and ISO-source first responses. The actual accepted
85-line buffer compiles with the selected CEM classpath; the expected superclass
`getBreakSpeed:()F` call is independently present in its bytecode. Both editor
clients acknowledge shutdown and exit normally; all 395 fixture inputs and
twelve absent paths remain unchanged.

Strict corpus exit remains **1**. Guava and Codec no longer fail, but Error Prone
annotations 2.18.0 and Gson 2.10.1 now encounter unsupported compiler metadata.
The library reports 35,939 files / 844,722 symbols, with two failed inputs and
two missing sources (Patchy and KotlinForForge). The eighteenth regression
receipts remain immutable. Next isolate the actual selected declarations in
these two POM chains without weakening argument-boundary checks.

Evidence: `ktlsp-source-encoding-20261011/cem-closure-followup-1/` and
`ktlsp-goal-20261010/nineteenth-fixed-1/` and
`ktlsp-cem-nineteenth-fixed-1-20261011/`. POM arrival watching, materialized-file
fast paths, concurrent materialization and compiler-generated API admission
remain open. No six-root IDEA or performance acceptance is implied.

### Twentieth checkpoint: compiler argument collections and source path admission

Actual Error Prone annotations 2.18.0 metadata clears inherited compiler arguments
with an empty `combine.self="override"`; Gson 2.10.1 uses `compilerArg` instead of
`arg` for String collection entries. The bounded reader now accepts an empty
override and validates every direct scalar collection entry independently of its
element name. Nonempty overrides, nested/attributed arguments, encoding options,
argument files and unresolved option selectors still fail. Parent uncertainty
is retained; this does not implement arbitrary Maven configuration merging.

The original reader tests and fourteen rejection controls remain unchanged.
Two new reader tests first fail against the old implementation. A third test
reproduces unsafe manually constructed coordinates reaching the legacy extracted
source fallback; the shared source entry point now validates coordinates before
any artifact/cache lookup or destination construction. The sentinel reproduction
never invokes extraction or recursive deletion. All three tests then pass;
focused coverage is 90 passed, zero failed and one existing ignored download case.

Twentieth-fixed-1 passes formatting, 1,097 workspace tests with three ignored
across 40 suites, and both release builds. All 312 inputs match before/after tests
and release. Server SHA-256 is
`4dacc46cf1cdd8d14054b55804c8304b93ba6abed0ee99aa507f142be83813fe`.
The fresh CEM replay passes the same seventeen semantic/UI/guard conditions and
all seven first responses. The actual accepted 85-line buffer compiles verbatim
against the selected 101 classpath entries, 156 project outputs and seven scripting
plugin artifacts; bytecode contains the expected superclass `getBreakSpeed:()F`.
Both clients acknowledge shutdown and exit zero without forced termination.
All 395 fixture inputs and twelve absent paths remain unchanged.

Error Prone and Gson no longer fail. The actual library index reports 36,040 files
and 845,935 symbols from 101 dependencies, with **zero failed inputs and two missing
sources**. Strict corpus exit remains **1** because Patchy 2.2.10 and KotlinForForge
Neoforge 5.12.0 sources are unavailable. Corpus shutdown takes approximately 3.71
seconds against the two-second target; the separate UI client takes approximately
1.65 seconds. Each is one sample, not performance acceptance.

Evidence: `ktlsp-source-encoding-20261011/cem-metadata-followup-2/`,
`ktlsp-goal-20261010/twentieth-fixed-1/` and
`ktlsp-cem-twentieth-fixed-1-20261011/`. Independent audit at
`ktlsp-cem-twentieth-proof-audit-20261011/receipt.json` checks the first responses,
whole buffer, compiler output, normal exits and all 1,430 guards. Earlier failed
receipts remain immutable.
Six-project replay, compiler-generated API admission, POM arrival watching,
materialized-file fast paths and concurrent materialization remain open.

### Twentieth corpus replay: partial and stopped after Spring

Nine runs use the immutable twentieth server and unchanged probe expectations.
Moshi passes 3/3 golden requests and all 29 incomplete-editing checks; Retrofit
passes 6/6 and 29/29, with exact facade and Java member targets. OkHttp's corrected
incomplete run passes 29/29. Coverage remains independent: Moshi has zero failed
or missing inputs; OkHttp has one failed input; Retrofit has six failed and three
missing inputs. These failures retain strict harness failure status.

Earlier direct-native CEM incomplete and OkHttp golden runs omitted `HOME`, unlike
the Bash setup. Their results remain preserved and cannot establish paired corpus
acceptance. The first CEM Bash monitor also missed client/server identities across
MSYS ancestry, so its memory is unmeasured. A separate regression reproduces native
Windows cache lookup failures with absent/empty `HOME` and a Gradle-home override;
all four child-process conditions pass after the shared home lookup repair.
Subsequent full/release/editor evidence is recorded in the twenty-first checkpoint below.

Spring passes its first four golden requests, but has ten failed and ten missing
inputs. Peak server commit is 4,930,125,824 bytes, above the prospective 4 GiB budget.
Client shutdown times out at fifteen seconds. A new owned Gradle daemon survives
the controller and is stopped; all owned processes are then absent. Safety memory
caps were not exceeded. The remaining Spring/Gradle and corrected setup runs are
held for a fresh release binding, rather than counted as passes.

The valid initial model is followed by 25 forced refreshes. Re-emitted model
content is identical after normalizing module order. Actual notifications include
read-time JAR changes and `.gradle` cache directories; the latter incorrectly match
the script suffix. A later native Neovim reduction proves access-time feedback;
that reduction does not yet supply a product fix. All 1,048 input guards match. Evidence is
`ktlsp-six-project-twentieth-native-20261011/{terminal-hold-after-run09,owned-absence-after-run09,run-09-refresh-cause-audit}.json`
and `ktlsp-native-home-draft-20261011/focused-red-green.json` under the recorded Temp root.
No six-project correctness or performance acceptance follows from this partial run.

### Twenty-first checkpoint: native Windows home lookup and directory notifications

Native Windows launches can omit `HOME`. The shared lookup now preserves an explicit
nonempty `HOME` and otherwise uses the operating system's home directory. Gradle cache
lookup honors a nonempty `GRADLE_USER_HOME`; Maven, download, dependency, SDK and timing
paths reuse the existing shared policies. The child-process regression fails before
the repair for absent/empty `HOME` and the Gradle override, then passes all four
conditions. A separate regression rejects `.gradle` cache-directory notifications
as build scripts while retaining actual `gradle/custom.gradle` refreshes.

Formatting, all 1,098 workspace tests (zero failed, three ignored, 40 suites) and
both release builds pass. The 312 guarded inputs match before/after tests, release
and independent review. Server SHA-256 is
`13fae4f535aca108ba40b8da6719a9da2dd8dd238bc1c3d8a8117e0b01dadb3e`.
The existing real-Neovim `basic` route passes nine assertions. Its forced stop and
MSYS ancestry leave graceful shutdown and client/server memory unproved for that run.

The direct-native CEM replay unconditionally removes both `HOME` and
`GRADLE_USER_HOME` before launching Neovim, retains the real `USERPROFILE`, and uses
a fresh isolated cache with the validated model seed. All six first responses use
one attempt; all 29 incomplete/unsaved editing checks pass. All 1,316 guards remain
unchanged. The index has 101 dependencies, 36,040 library files, 845,935 symbols,
zero failed inputs and the same two missing sources, so the strict harness retains
exit 1. Shutdown receives one null acknowledgement, sends one exit and the actual
server exits zero without forced termination; all owned processes exit. Its 2.614
seconds still exceed the two-second budget. This is one run, not latency acceptance.
An earlier replay also passes the editing checks but lacks unconditional environment
removal; its `HOME` condition remains unproved rather than being promoted to a pass.

The separate access-time reduction runs installed Neovim 0.12.5/libuv 1.52.1 on
disposable files. A passive read after assigning an old access time produces both
a raw change event and the actual `vim._watch` Changed callback while content hash,
size and last-write time remain unchanged. All 24 assertions pass, including real
write and rename controls; all owned processes exit. This establishes the feedback
cause. No Changed-event suppression or native replacement is applied yet: real
concurrent writes and notification failure must remain conservatively invalidating.

Evidence under the recorded Temp root: `ktlsp-goal-20261010/twentyfirst-fixed-1/`,
`ktlsp-native-home-draft-20261011/focused-red-green.json`,
`ktlsp-project-watch-filter-20261011/{red-cargo,green-cargo}.log`,
`ktlsp-home-twentyfirst-20261011/root-audit.json` and
`ktlsp-watch-atime-reduction-20261011/causal-audit.json`.
Next: remove the access-time refresh feedback without losing writes or overflow
invalidation, then resume the held Spring/Gradle editor replays with a fresh binding.

### Twenty-fourth checkpoint: admitted compiled-callable foundation

Compiler facts can now enter the same contribution store through an exact scoped
authored Java owner. Providers receive opaque session-bound class/method handles;
binary changes, unsaved source replacement and model changes invalidate positive
and negative reads, committed contributions and pending work. Equal decoded inputs
remain a no-op. Four public-API regressions use the committed real classfile fixture
to exercise origin/scope separation, cross-session rejection, revision/CAS controls
and unsupported shapes. Altered normalized metadata in negative controls is not a
successful-compilation oracle.

The admitted surface is deliberately limited to public, nonstatic, nongeneric,
zero-argument Java methods returning `java.lang.String`. Exact duplicate provider
facts describe one callable without merging provider ownership; conflicting facts
remain Unknown. Known inherited overrides are dominated without deleting other
overloads or unknown competitors. Java result inference, hover, signature help and
incomplete member completion require the same value-receiver proof. A type spelling,
an unresolved imported value and an incomplete parent never establish an instance.
Generated methods have no invented source declaration. An authored ancestor rename
is refused if it would leave an admitted generated override unchanged and alter
polymorphic dispatch. Other overloads, names, owners and scoped parents retain
applied and whole-buffer-inspected edits.

The first core and four reader regressions have actual failing runtime assertions
before their repairs. Dispatch verification separately preserves two erroneous
signature-label/count expectations and their corrections; those failures are not
reported as product defects. Two incomplete-completion regressions and three
rename/name-fallback regressions also fail at runtime before repair. A further
independent review found mixed applicable candidates could lose the generated
candidate when navigation filtered missing source ranges. Two actual navigation
assertions fail before the shared gate repair and pass afterward; the reader group
now passes 14 tests. An additional authored-only private-call control verifies exact
navigation/references and an applied whole-buffer rename. The four public-API tests
also verify that the compiled-presence cache retracts with contributions while an
immutable snapshot retains its original state.

The valid private-ancestor case conservatively remains Unknown. Preventing a false
ancestor target is a consistency repair; useful selection of the generated method
still requires visibility-aware candidate filtering and remains a coverage gap.

Twentysecond-fixed-1 passes 1,111 tests and twentythird-fixed-1 passes 1,117 tests,
each with zero failures, three ignored, 40 suites and unchanged 312-input guards.
Both release builds succeed. They are preserved intermediate checkpoints. Final
twentyfourth-fixed-1 passes all 1,119 tests, zero failed, three ignored, 40 suites,
with formatting and release builds successful. Its 312 guarded inputs match before
and after tests, release and editor replay. The server SHA-256 is
`bfac0eb253a4b366bf7517ce038a01ae703dafbb7b59cbeafaf310ad2c593a2b`.

The same immutable release runs the unchanged existing Neovim Java and semantic
routes. Java passes 191/196 with exactly the five previously recorded diagnostics
still failing. Semantic passes all 23 checks, acknowledges shutdown once, sends one
exit and records actual server exit zero with no forced termination. Its 4.439
seconds exceed the two-second budget. Both monitor receipts preserve all 633 input
guards. MSYS ancestry did not capture actual Neovim/server handles, so client/server
RSS and complete native process ownership remain unmeasured for these runs; the
observed wrapper processes all exit. No latency distribution or steady-state memory
claim follows. The first editor preparation confused a receipt JSON hash with the
monitor Python hash and stopped before native launch. That attempt remains preserved;
the fresh retry verifies the script against its original successful-run guard.

Evidence under the recorded Temp root:
`ktlsp-admitted-compiled-foundation-20261011/`, including `dispatch/`,
`ktlsp-compiled-input-public-api-draft-20261011/`,
`ktlsp-compiled-override-rename-draft-20261011/current-rebase-2/`,
`ktlsp-compiled-resolution-fallback-draft-20261011/`,
`ktlsp-compiled-navigation-gate-draft-20261011/`,
`ktlsp-goal-20261010/{twentysecond-fixed-1,twentythird-fixed-1,twentyfourth-fixed-1}/`
and `ktlsp-compiled-foundation-twentyfourth-editor-20261011/retry-1/`.

The default server has no connected native compiler producer. Attaching decoded
facts is a trusted-host operation requiring a verified successful build and fresh
input/output ownership; these API tests do not supply that verification. Kotlin
generated-call/property projection is also open. All twelve transformed-API editor
failures remain failures. Native process ownership and Windows notification drafts
are unapplied and uncompiled, access-time feedback is not fixed, and no six-project
correctness, latency or memory acceptance follows from this slice.
Next: establish the owned native build lifecycle and conservative invalidation,
retain declared-task-input/output categories before flattening, reject overlapping
or unproved processor ownership, then connect only successful fresh compiled inputs.
The held native process and notification drafts must be compiled and exercised
before use. Resume the real-project and compiler-plugin editor failures afterward.

### Twenty-fifth checkpoint: owned Windows Gradle execution

Windows Gradle launches now assign their suspended root to a retained, non-inheritable
Job before resuming its verified primary thread. The same blocking worker retains
the exact root, Job and both output readers until root termination, zero assigned
live processes, EOF and reader joins are verified. Timeout, cancellation, unwind
and failed setup clean up that owned tree. No existing Java process is adopted or
terminated. OS termination/query/EOF failures keep the worker quarantined rather
than releasing ownership or claiming bounded shutdown.

The public compiler runner and actual Gradle classpath discovery share this guard.
They use `--no-daemon`; a successful wrapper has a two-second natural descendant
shutdown grace within the execution deadline. Forced cleanup and truncated/read-error
output preserve parsed diagnostics but cannot clear previous compiler errors or
publish a partial project model. This does not certify a compiler read closure,
successful transformed API, or reuse of an existing external compiler daemon.

A real fake-wrapper regression fails before the repair: a successful root leaves
a silent descendant holding inherited pipes, return takes 20.0508 seconds and
`executed=true`. After repair the same public path returns in approximately 2.23
seconds, the retained descendant handle is terminal, its warning is preserved and
`executed=false`. This is a process-lifecycle regression measurement, not completion
latency. Other native controls cover normal/error exits, a natural daemon tail,
timeout/cancellation, both noisy streams, unwind and failure before resume.

Independent review caught a second regression before commit. The accepted
55,170,313-byte, 194-module/1,176-source-set Gradle model requires at least
45,259,017 protocol bytes for CP, SCP and ARTIFACT records alone. Reusing the
compiler's ten-MiB capture budget would reject a valid cold dump. Classpath stdout
therefore has a separate 64-MiB bound; compiler output and classpath stderr retain
ten MiB per stream. Readers continue draining after the bound and reject truncation.
Valid UTF-8 capture is parsed without copying through another raw-output buffer.
The frozen model is `ktlsp-server-memory-pair-20261011/cache/classpath/8460759510824a32.json`,
SHA-256 `e3ab7b955006e1b4871c747b734ed3f1b2642dd56472d613005689bc00b63da4`.

A new actual fake-wrapper/model-loader regression fails at runtime with the old
capture budget. After repair it accepts every record and the late second module
in an eleven-MiB dump, while a dump over 64 MiB remains an error. Two subsequent
fixture corrections are retained separately: the Rust test runner prefixes the
first line, requiring a newline before PROJECT; parsed CP entries preserve
duplicates, so the expected record count is 11,038. Neither correction is a new
product defect. Fitting the entire real 194-module dump under 64 MiB still needs an
actual cold/forced replay; the recorded size is a lower bound.

Final `twentyfifth-fixed-3` passes 1,127 Rust tests, zero failed, four ignored across
40 suites; formatting and the release build succeed. All 312 guarded inputs match
before/after tests, release and editor replay. Its server SHA-256 is
`8b0330d574b71acb8df1b374501bbd61fc64105058ea406e3bf4be00de96c9e7`.
The unchanged Neovim library route passes 13/13 and creates a fresh model-v4 cache
with both dependencies and zero unresolved entries. Semantic passes 23/23, records
one acknowledged shutdown, one exit and actual server exit zero without forced
termination. Shutdown takes 4.192 seconds, still failing the two-second budget.
Both editor receipts preserve 633 guards. MSYS ancestry again observes wrapper
handles only, so actual server/client RSS and complete native process ownership
are unmeasured in these editor runs. The private Job tests supply their own exact
retained-handle proofs; they do not repair this monitor limitation.

Evidence: `ktlsp-native-compile-ownership-20261011/root-1/`,
`ktlsp-goal-20261010/twentyfifth-fixed-3/` and
`ktlsp-native-compile-twentyfifth-editor-20261011/final-3/root-audit.json`.
The first twentyfifth checkpoint (1,126 tests) is preserved; fixed-2 was prepared
but never launched. Final3 has corrected base-revision metadata.

The ordinary default Kotlin compiler-daemon strategy, including preexisting-daemon
reuse, has no real positive control for this guard yet. A persistent descendant
could force cleanup and conservatively leave `executed=false`. Unix keeps its legacy
wrapper-only process ownership and has no native proof here. The native compiler
producer and notification backend remain unconnected; transformed-API editor
improvement and access-time-feedback improvement remain zero. The first Temp
notification candidate passes 13 Windows tests and feature compilation checks.
Its supplemental snapshot passes 14 default tests (two pure, five synthetic, six
native API and one public layout/trait check). One separate, explicitly selected
kernel-overflow test passes on its only attempt: 2,048 writes produce one native
Rescan, zero native errors and no expired callback barrier. Both supplemental
runs preserve all 336 guarded files; every observed owned process exits and all
retained handles close. Evidence is
`ktlsp-compiler-producer-20261011/root-1/notify-runtime-audit.json`.
These are standalone prerequisite checks. Consumer retraction and product
access-time-feedback behavior remain unproved; the watcher is still unconnected.

Next reproducible command: `rtk proxy cargo test --locked -p ktlsp --lib compile:: -- --nocapture --test-threads=1`
with the recorded Rust homes/toolchain and a disposable `KTLSP_CACHE_DIR`. Then run
real Gradle positive/default-daemon controls, close conservative source/config/binary
invalidation and connect only fresh successful compiler facts. Missing prerequisites
are an isolated default-daemon control fixture and the verified native watcher/host
admission path. The original twelve transformed-API failures and six-project IDEA,
latency and memory acceptance remain open.

### Twenty-sixth checkpoint: reject empty and interrupted benchmark successes

The diagnostics benchmark could report successful recovery when its backend returned
an unexecuted empty outcome. It also accepted matching empty oracle results, and
partial error output could pass injection without completed execution. The shared
probe-error check now requires execution. Recovery additionally requires a genuine
executed error during setup and an executed clean result; both oracle arms must
establish the injected error before their diagnostics can pass comparison. Warmup
remains discarded and the report schema is unchanged.

Tests-only reproduction fails four assertions with native exit 101: 28 pass and
four fail, without a compilation failure. The corrected focused suite passes all
32 tests, including five recovery setup/result combinations and the public oracle
route on a non-Gradle directory. Evidence is `ktlsp-bench-validity-20261011/{red,green}.{out,json}`.

`twentysixth-bench-fixed-1` passes 1,130 workspace tests, zero failed and four
ignored across 40 suites; formatting and release builds succeed. All 312 guarded
inputs match before/after tests and release. The only change within those inputs
since final3 is `crates/ktlsp/src/bin/bench.rs`. The server binary is byte-identical
to final3, SHA-256 `8b0330d574b71acb8df1b374501bbd61fc64105058ea406e3bf4be00de96c9e7`;
its existing library 13/13 and semantic 23/23 evidence remains the editor check.
No fresh editor run or new editor acceptance is claimed for this measurement-only
change. The corrected bench SHA-256 is
`ae52924a4e6fc58032d10acea341b88f7f98abc81f6662b6746e265f9d3f627e`.
Evidence is `ktlsp-goal-20261010/twentysixth-bench-fixed-1/root-audit.json`.

Older benchmark success alone does not prove `executed=true`; rerun affected
controls using the corrected binary. Native generated-API admission is still
unconnected. Source review also found a pending producer must reject observed
read inputs outside the watch coverage actually armed before its build; later
watch extension cannot prove that an input avoided A-to-B-to-A changes during
compilation. This is a static finding, not an executed exploit or a product fix.
Next: finish that guard and its focused tests, then invoke a fresh Java-only Lombok
positive through the existing corpus harness and current model. The default Kotlin
daemon control, mixed upstream-output reads and original twelve transformed-API
editing failures remain open.

### Twenty-seventh checkpoint: observed Java builds and selected source roots

An executed compiler task alone did not prove its generated API reached the current
editor snapshot. Full-2 and full-3-debug native replays both retained an empty first
`getName` response after `:compileJava` executed and publication rejected stale inputs.
The historical failures remain immutable.

| Check | Observed result | Status |
| --- | --- | --- |
| Full-3-debug Rust/release | 1,159 pass, zero fail, four ignored, 40 suites; native 0 and 361 inputs unchanged | PASS for that snapshot |
| Full-2 ordinary editor | basic 9/9, features 78/80, library 13/13, Java 191/196, semantic 23/23 | PARTIAL; same two/five completeness-policy failures retained |
| Selected-source-root lifecycle tests | Eleven pass | Focused PASS |
| Full-4 Rust/release | 1,162 pass, zero fail, four ignored, 40 suites; native 0 and 363 inputs unchanged | PASS for that snapshot |
| Full-4 durable fixture generation | Native 0; ten files match original, except the explicit portable Java toolchain properties | Setup PASS |
| Full-4 observed Java corpus | 99/99 checks, including 89 callback checks; initial getter useful on its first response | Saved pure-Java PASS |
| Independent terminal audit | 757 capture-time guards and 43 artifacts unchanged; four raw compile sequences and normal LSP exit verified | PASS within stated limits; full OS process-tree/RSS measurement remains open |

The first invalidator is confirmed: `10:05:57.273572Z Create(Any) project/build`
preceded client invalidation at `10:05:57.395`. The watcher included unrelated source
sets, including a missing test annotation-processor output root. Creating the main
build directory therefore invalidated the selected main Java compilation. The native compiler-input watch now follows the selected main Java task.
Existing workspace invalidation still covers same-module main/test/external source edits;
foreign modules and same-named included-build modules are excluded by exact ownership.
Eleven focused tests preserve these boundaries. Passive read/atime is not the proven cause.

The durable probe reuses the corpus client: `dev/compiler_plugin_project.lua --java-only`,
`dev/corpus/observed-java.json` and `KTLSP_CORPUS_EXTRA_PROBES=dev/nvim_observed_java.lua`.
Person/Consumer raw bytes and expected authored control are retained. The callback and
manifest match after LF normalization; guards bind their actual CRLF hashes. The
fixture uses Java 17/Lombok 1.18.32, one worker, a 384 MiB heap, no persistent Gradle
daemon and no VFS watcher, with no Kotlin plugin/strategy override. Default mixed
fixture generation is unchanged.

The generated fixture has ten files: original Java/config/wrapper raw bytes match,
including 27-byte `lombok.config` SHA `cef77f6e12c36c1efc67602e98e6012c742c9a2ccc552dc0df3334b670a024ef`.
Only portable `org.gradle.java.installations.fromEnv=JAVA_HOME` differs. The binding
records the post-full-4 fixture-only config entry and unchanged compiled Rust inputs.

Four phases return `observed-success`, `executed=true`, `:compileJava`, zero diagnostics
and `compilerClosureComplete=false`. The first getter query returns exactly one
`getName`; hover is String and result-chain completion offers `length`.
`initial_getter_queries=1`, `semantic_retry_attempts=0`, and four verified outbound
edit barriers preserve first-response freshness. Configuration changes and provider
disable retract facts; the disabled command and two dirty-source commands are rejected.
A-to-B-to-A restoration or re-enable alone cannot revive a receipt; a new compilation
is required. Configuration here is standard `didChangeConfiguration`, not a native
filesystem test of `lombok.config`. Source-less goto/references/rename stay unavailable.

Shutdown records one null ACK, one exit notification, native server 0, no terminate
and 0.8926658 seconds (n=1). The monitored wrapper exits 0, all 757 before/after guards
match, and its four captured RTK/Bash processes are terminal; three retained handles
are opened/closed. MSYS hides actual Neovim/server roles, so their RSS/cap enforcement
is NOT_MEASURED. The independent audit verifies capture-time hashes and raw observations;
it does not independently retain every OS descendant. These observations do not establish
p50/p95 or a memory/latency acceptance result.

Full-4 server SHA-256 is
`24972c7a29de66612ed4bef8a5231153a3a2b91d942fb88de5a6668450dd503f`;
bench is `71115765c644ad9da51751c7cad90b4ec35edb977b70c91092bb3d68d106b046`.
Build evidence is `ktlsp-compiler-producer-20261011/product-1/full-{2,3-debug,4}`;
ordinary editor evidence is `ktlsp-compiler-producer-full2-smokes-20261011`.
Native evidence is `ktlsp-compiler-java-clean-binding-20261011-3-selected-roots/`
`{binding.json,monitor-corpus.json,editor-root-1/artifacts/}`.
Independent audit: `ktlsp-compiler-java-clean-audit-selected-roots-20261011-1/`
`audit-receipt-2.json` (SHA-256 `53a99f2b4ae30cf897d45659ab64a026b4d2aa9b85b9e79a49bacdde643e7b85`).

The matched default Kotlin daemon benchmark still fails injected-error and recovery
checks (n=1); the in-process control passes both. Default really used newly owned
Kotlin daemons, while recovery skipped compileKotlin as up-to-date. Per-call interrupted
outcomes were not captured, so forced daemon cleanup remains a hypothesis. Changing
the compiler strategy is not the product fix. Evidence: `ktlsp-native-gradle-default-control-291a6df-20261011/`
`{audit-default-readonly-1.json,native/root-matched-arm-audit.json}`.

Existing features/Java failures follow the conservative producer-completeness policy;
the default-false guard is unchanged. Compiler-producer closure remains unknown.
Useful generated completion after unsaved edits, the original mixed twelve cases,
six-project IDEA parity and latency/RSS acceptance remain OPEN. Production Mixin
support belongs in a separate project; extension loading/ABI remains OPEN.

The then-unapplied request-path reuse proposal is implemented and measured in
checkpoint 28 below. Earlier source-only observations are not performance evidence.

### Twenty-eighth checkpoint: prepare immutable compiler paths once

Compiler resource checks rebuilt watch paths and repeatedly validated the whole
input/output role set. Requests now prepare normalized output/watch keys while
exclusively owned, before Arc publication. Notifications borrow that immutable plan.
Native compilation still freshly validates roles and checks the prepared environment
before and after its input inventory. Read coverage, negative guards, post-build
hashes, dirty-source rejection and final publication checks remain in place.

Four new regressions cover source/config/toolchain/processor/plugin/classpath
boundaries, failed preparation clearing an old plan, all nine prearmed input roles,
and retraction of an actual admitted getter after configuration A-to-B-to-A.
The telemetry test now checks its explicit-path helper without changing process-wide
environment variables. The former test could race environment hashing in parallel
tests; this is a source-level race finding, not a reproduced product failure.

The first full run fails one new getter regression: the test omitted the existing
publication step after queuing compiled facts. Only the test adds
`bump_index_revision()`; its useful-getter 1-to-0 expectation is unchanged.
`full-1` and `test-publication-correction.json` preserve that failure.
Fresh `full-2` passes 1,166 tests, zero failed and four ignored across 40 suites.
Formatting and locked/offline release builds pass; all 363 product inputs match
before/after tests and release. Evidence root:
`ktlsp-path-cache-slice-20261011-1/`.

The allocation experiment is a Temp-only ignored release test, not a new product
benchmark framework. Both arms use identical instrumentation apart from the setup
hook and frozen path/event expectations. Timing and allocation passes are separate;
each completed row has 30 timing batches and 30 allocation batches, 32 events each.

| Request shape | Before | Prepared paths | Status |
| --- | --- | --- | --- |
| Actual exported Java task, 40 watch keys | 23,427,148 requested allocation bytes/batch; p50/p95 33.8522/36.7479 ms | 0 requested bytes/batch; p50/p95 0.0031/0.0032 ms | Paired instrumented local API result |
| CEM-sized synthetic CP101, 142 keys | Watch keys/events match; timing-loop deadline fails before a completed row | 0 requested bytes/batch; p50/p95 0.01655/0.021 ms | Candidate only; no paired timing claim |
| Gradle-sized synthetic CP212, 255 keys | NOT_RUN after CEM failure | 0 requested bytes/batch; p50/p95 0.0115/0.0189 ms | Candidate only |

Java retained requested bytes rise from 8,286 to 14,463 (+6,177), including 6,105
cache bytes and 72 request-storage bytes. Candidate CEM/Gradle cache setup retains
26,355/49,711 requested bytes; those shapes have no completed baseline allocation
row. Every completed row frees its tracked allocations after drop. These counters
measure System allocator requested bytes, not allocator overhead or OS RSS.
Peaks reset per phase; constructor/setup/first-call measurements are n=1.

The baseline v1/v2 native-101 failures remain immutable. V2 exports the completed
Java row before CEM exceeds a 120-second iteration assertion; its native test
takes 139.87 seconds, so this is not a hard process timeout. Candidate v2 completes
all three shapes. The allocator TLS check remains installed during timed passes,
and execution order is fixed baseline then candidate. This is not uninstrumented
product, Workspace/latch, watcher delivery, inference, RPC, UI or compilation timing.
CEM/Gradle requests are structural model-derived fixtures, not real-project runs.

Measurement evidence is `measurement-1/{baseline-v2.out,candidate-v2-results.json,root-measurement-audit-2.json}`.
Candidate results SHA-256:
`04fa977fd2185b46e0f44bc1c0bb425f1bc807827f020a5189b2f098d48254be`.
The independent audit passes 70 checks and preserves 387 frozen input hashes:
`ktlsp-path-cache-measurement-review-20261011-1/audit-receipt.json`,
SHA-256 `1d29bff709efc8aa6bcf4df78fa027d78e074c1e862a29f18dad9aef1d8d5a5d`.
The corrected root receipt labels retained requested bytes explicitly; its earlier
receipt remains preserved. No resident-memory reduction is claimed.

A fresh real-Neovim corpus run passes 99/99, including all 89 callback checks.
All 38 semantic requests receive their first response with one attempt, zero
semantic retries and one initial getter query. Four distinct raw observer sequences
prove selected `:compileJava` execution, stable inputs/outputs and zero diagnostics;
all four report `compilerClosureComplete=false`. Dirty/configuration retraction,
provider disable/re-enable and fresh compilation expectations are unchanged.

Normal LSP shutdown has one null ACK, one exit notification, native exit 0, no
forced termination and 0.7201557 seconds (n=1). At capture time, 757 guards and
43 artifacts remain unchanged; four monitored wrappers terminate and three retained
handles close. MSYS still hides actual Neovim/server process roles, so complete
OS descendant ownership and their RSS/cap enforcement remain unmeasured.
Configuration control is explicit `didChangeConfiguration`, not a native config-file
watcher test. Useful generated completion during unsaved edits remains open.

Current server SHA-256:
`a0291541cfa88518aab742e22d87259611281eb9324eb612ed73e25f03d84e0e`;
bench SHA-256:
`3eec7702799f7d8bdda068cccc39894c695295071066be9b25054e1f570fdf21`.
Native evidence: `native-java-1/{monitor-corpus.json,editor-root-1/artifacts/}`.
Root terminal audit: `native-audit-1/audit.json`,
SHA-256 `1294092accb8a00466dd9aa25399c5eff8febce0f33d91083c9a9ef70b006fc8`.
The product/full-run guards apply before these later documentation edits.

Next reproduce/fix the default Kotlin daemon control without changing the user's
compiler strategy, relaxing executed/EOF/whole-build gates or terminating existing
daemons. Review evidence is `ktlsp-owned-kotlin-daemon-review-20261011/`;
the matched failing control is
`ktlsp-native-gradle-default-control-291a6df-20261011/native/root-matched-arm-audit.json`.
The exact next native command requires a reviewed cooperative shutdown/ownership
seam first; source review is ongoing. The original mixed twelve editing cases,
KAPT/KSP stale diagnostics, six-project IDEA comparison and useful-result latency/RSS
acceptance remain OPEN. Production Mixin remains a separate extension project.

### Twenty-ninth checkpoint: extension receivers and unfinished selectors

Unqualified `this` now uses the shared implicit receiver stack, retaining an unknown
inner receiver instead of guessing an outer class. Kotlin completion supplies recovered
facts before receiver inference, replaces only the queried file, reapplies its project
scope and maps declaration names back to authored bytes for provider relations.
Intermediate selectors select their own receiver. Closed erroneous files use the same
recovery route; the canonical index and source buffers are not rewritten.

The independent javac/Kotlin 1.9.23/Java-caller/bytecode oracle passes four native stages
on the durable reduction. Workspace tests pass **1,177 / 0 failed / 4 ignored**; formatting
and release build pass. The eleven new guards include ambiguous foreign overloads,
module visibility changes and an enabled/disabled source-provider relation after the cursor.

| Existing Neovim expectations | Before | Current |
| --- | --- | --- |
| Reduction golden requests | 7/10 first responses | 10/10 first responses |
| Reduction incomplete requests | 1/2 first responses | 2/2 first responses |
| Reduction client checks | 13/16 and 16/17 | 17/17 and 18/18; each adds the in-repository Git-status guard |
| Real Spring controls plus four unsaved cases | Baseline readiness times out; new queries not run | 9/9 first responses; overall 33/35 checks |

All current requests use one attempt, with delivered/restored unsaved versions and
unchanged disk bytes. Reduction shutdowns are clean/native 0 in 0.774/0.801 seconds
(one sample each). Real Spring indexes 9,102 project files and 412 dependencies but
retains **10 failed/10 missing sources**, then requires forced termination after a
15.360-second shutdown timeout. Its baseline has watch-driven model supersession;
the current arm runs after shared Gradle caches have warmed, so startup speed is not
a paired measurement or a demonstrated benefit of this semantic fix. No whole-server
RSS or six-project performance acceptance is claimed.

Evidence base: `C:/Users/gummy/AppData/Local/Temp/ktlsp-spring-this-20261011-1/`:
`compiler/`, `full/`, `baseline-{golden,incomplete}/`, `native-{golden,incomplete}/`,
`spring-{before,after}-2-editor/` and `semantic-summary.json`. Initial full-root data
preflight failures are preserved as `spring-{before,after}`; no LSP query ran there.
Current immutable server SHA-256: `74a3059b1cf19cafd6828927fc5016c4d1906493020798da3e65803649cef476`.
The subsequent user correction withdraws timeout/source-availability gates as described
in Scope and Measurement policy. Next address reproduced editing/analysis gaps and
complete matched IDEA comparisons. Shutdown and watch supersession remain investigation
notes; the current Spring semantic queries pass, while broader cases are unverified.

### Thirtieth checkpoint: obsolete Java dependency members after a live build

The existing real-Neovim Gradle reload scenario reproduces two missing diagnostics:
after `vendor.Versioned` changes from `oldApi()` in v1 to `newApi()` in v2, both
languages update completion and exact definitions, but neither diagnoses the dirty
buffer's old call. The current shared negative query already handles both languages;
the global compiler-producer uncertainty prevents it from proving absence.

Keep that global uncertainty. For a Java caller with one captured standard JavaCompile
configuration and no opaque arguments, processor path, fork overrides or discoverable
processor/javac-plugin service, inspect the actual ordered compiler classpath. Its
first class match must be the selected dependency origin and current artifact revision.
The shared core checks normalized classfile identities, fields, methods and every
superclass/interface, including Object. Missing ancestors, annotated unknown surfaces,
cycles, ambiguous owners and affected multi-release classes remain unknown. Raw present
names suppress errors without asserting source callability. A new dirty buffer within
the same source set shares its compiler context; it need not have existed at the build.

Inventories are lazy and retain decoded queried-class facts rather than entire JARs.
The cache has a conservative 8 MiB metadata estimate and 4,096-entry limit; model
publication/refresh clears it, and lookups recheck file stamps. Directory service files
are rechecked directly. This is a cache policy, not a measured process-RSS result or
proof against unobserved same-stamp replacements. Direct file-entry forwarding also
prevents static Java receiver diagnostics from scanning unrelated index files.

Evidence root: `C:/Users/gummy/AppData/Local/Temp/ktlsp-java-binary-absence-20261011-1`.
The unchanged 76 editor expectations run with Neovim 0.12.5, Gradle 9.6.1,
Kotlin 2.4.0 and JDK 17's actual `lib/src.zip`, in disposable projects/caches.
The prior accepted release (`74a3059b1cf19cafd6828927fc5016c4d1906493020798da3e65803649cef476`)
passes 74/76; the new release
(`3330b973b51877beec602eb2f36e45bbdf377fe3db135df8ffb6ffac6258d92c`)
passes 75/76. Java returns the precise `oldApi` unresolved-reference range; Kotlin
still returns an empty diagnostic list and fails that assertion. All four builds,
updated completion/definitions, preserved unsaved changes, six current-valid buffer
diagnostic checks and clean shutdown pass. Native notifications drive the updates;
the test sends no explicit watch notifications. Harness/user-file hashes are unchanged
within both arms. This is an eventual-refresh correctness check, not an IDEA latency
or UI-display comparison. Raw receipts and responses are under `native-{before,after}`.

`compiler/results.json` independently checks all four v1/v2 old/new Java calls with
javac 17: exactly the expected two compile and two fail for the missing method.
Two javac 21 controls verify that `Thread.destroy()` compiles with `--release 8`
and fails against its current API. Therefore the JDK inventory retains unknown
for explicit `--release` until ct.sym is supported, instead of substituting jmods.
`cargo test --workspace --locked --offline` passes **1,190 / 0 failed / 4 ignored**
across 40 suites. Formatting and release build exit zero. Thirteen new tests cover
caller configuration, binary origins/hierarchy, missing inputs, multi-release classes,
service/configuration drift, bounded retention and avoiding unrelated global scans.

Next reproduced gap is the unchanged Kotlin `oldApi()` diagnostic assertion. Reproduce
with `KTLSP_GRADLE_RELOAD=1 KTLSP_GRADLE_RELOAD_AUTOWATCH=1` and
`KTLSP_JDK_SRC="$JAVA_HOME/lib/src.zip" dev/ktlsp-harness.sh gradle-live --release`.
Do not apply the Java inventory proof to Kotlin before accounting for visible extensions
and its compiler transformations. General plugin/processor absence, ct.sym, broad
six-project IDEA parity, request/UI latency and whole-process RSS remain open.

### Thirty-first checkpoint: capture Kotlin compiler task inputs

Model v5 marked Kotlin compiler configurations complete while exporting no source
files, classpath or default JDK. This reproduces on Gradle/KGP 9.6.1/2.4.0,
8.10.2/1.9.23 and 7.4.2/1.8.21. KGP's serialized arguments intentionally omit
these inputs; generic declared task inputs also contain compiler tooling and cannot
substitute for the configured library classpath.

Read Kotlin, Java and script source collections, ordered `libraries` and the
destination directory from that task. Its selected `buildJvm.javaHome` supplies
the default JDK; explicit `-jdk-home` takes precedence and `-no-jdk` leaves no JDK.
Duplicate or relative JDK overrides remain incomplete. The two required KGP
internal getters were checked against all three cached primary implementations;
unsupported layouts retain partial facts with `complete=false`. Model v6 prevents
the old incomplete tuples from satisfying the disk cache. The existing cache test
also preserves source paths, classpath order, selected JDK and opaque arguments.

Evidence root: `C:/Users/gummy/AppData/Local/Temp/ktlsp-kotlin-compiler-inputs-20261011-1`.
All three fixture builds and cold/warm configuration-cache dumps exit zero. Main
Kotlin task source/classpath counts change from 0/0 to **3/3**, **6/4** and **6/8**,
respectively. Captured arguments remain identical to the baseline, including
serialization, all-open, no-arg and Lombok plugins in 1.9 and KAPT/KSP in 1.8.
Cold/warm records are identical and each warm run explicitly reuses its cache.
The KAPT processing task retains its separate processor/options record; this
slice does not fill its source/JDK tuple or certify compiler transformations.

Independent controls run Gradle on JDK 17 and select JDK 21 for only the Kotlin
task. The model reports 21; `SequencedCollection` fails compilation under 17 and
passes under 21. An explicit JDK 22 override also compiles that API. The first
toolchain-control attempt exposed KGP's deprecated setter also changing JVM target;
the corrected control restores target 17 before compilation. A custom-input build
compiles an external Kotlin file, excludes deliberately invalid Kotlin and keeps
`z-first.jar` before `a-second.jar` in the captured task classpath. An unsupported
task getter stays incomplete without losing Java source ownership. Receipts are
`matrix-assertions.json`, `jdk-api-results.json`, `configured-inputs.json` and
`unsupported-task.json`.

The immutable before/after releases have SHA-256
`3330b973b51877beec602eb2f36e45bbdf377fe3db135df8ffb6ffac6258d92c` and
`b65411c484a6a45fbac6e0d21e0cafc100a0109dfb4cdf847eb8ae2f93a31e4d`.
The same real-Neovim automatic-watch probe retains all 76 editor expectations and
adds ten configured-input checks before/after the v1-to-v2 dependency switch.
Those ten improve **0/10 to 10/10**. The original checks remain **75/76**; Kotlin's
obsolete `oldApi` diagnostic still fails. Both arms complete all four actual builds,
preserve dirty Java/Kotlin behavior and exit cleanly. Every harness/user-file guard
matches across both arms, with zero explicit watch notifications. The resulting
75/86 to 85/86 probe totals describe these checks, not project-level acceptance.
Workspace tests pass **1,190 / 0 failed / 4 ignored** across 40 suites; formatting
and release build exit zero. No latency or process-RSS result is claimed.

Next reproduce Kotlin's unchanged diagnostic with
`KTLSP_GRADLE_RELOAD=1 KTLSP_GRADLE_RELOAD_AUTOWATCH=1
KTLSP_JDK_SRC="$JAVA_HOME/lib/src.zip" dev/ktlsp-harness.sh gradle-live --release`.
Configured inputs and opaque arguments do not prove effective CLI semantics,
compiler-producer closure or complete visible Kotlin extensions. The alias/local
extension candidate reductions need compiler/editor reproduction before widening
negative diagnostics. Broad six-project IDEA parity and matched performance
measurements remain open.

### Thirty-second checkpoint: imported Kotlin extension aliases

The compiler-valid `Versioned.compat()` extension imported as `oldApi` was omitted
from qualified member resolution, inference and completion: candidates compared
the written alias against the authored name. The shared resolver now enumerates
visible call-site names while preserving the original declaration, package and
definition range. Generic receiver extensions query the real imported name bucket.
An alias hides the original name even from same-package and wildcard visibility;
an explicit unaliased import restores it. No synthetic alias declarations enter
the global index. Completion uses these same names, keeps inherited real members
ahead of extensions, and call-shape queries use shared typed applicability.

References discover aliases only in candidate Kotlin files from the existing
reverse index, resolve their actual import targets and re-resolve usage sites.
Authored rename changes declarations, direct calls and import targets while
preserving alias declarations and alias calls. `prepareRename` returns the cursor's
file/range. Closed-file queries, unsaved import rebinding and close-to-disk recovery
are covered. Ambiguous overload imports, same-spelling aliases and candidate-cap
overflow currently refuse rename; import splitting and alias-binding rename remain
coverage gaps, rather than partial edits being returned as a successful rename.

Evidence root: `C:/Users/gummy/AppData/Local/Temp/ktlsp-kotlin-extension-alias-20261011-1`.
The four portable sources are under `dev/semantic-fixture/probe/extensions/`.
Gradle/KGP **8.10.2/1.9.23** and **9.6.1/2.4.0**, running on JDK 17, compile the
original, rebound and authored-renamed versions. The rebound version left with the
wrong receiver's `payloadOnly(): String` fails with an actual `Int` return mismatch.
Separate compiler controls confirm alias hiding, explicit restoration, multiple
aliases, removed imports and unrelated receivers. Commands and compiler output are
in `compiler-*-results.json` and `compiler-*.log`.

The immutable before/after release SHA-256 values are
`b65411c484a6a45fbac6e0d21e0cafc100a0109dfb4cdf847eb8ae2f93a31e4d` and
`9e318a840222c184c2cea3c89bc0103615c6d9947fbeaed5ebb21e061751b6fc`.
The same real-Neovim `semantic` scenario retains its 23 checks and adds 42 alias
checks. Alias checks improve **30/42 to 42/42**; the existing 23 pass in both arms.
All **18 first requests** receive protocol responses in each arm, with no protocol
errors or retries. The checks include incomplete `v.old`, exact typed return-chain
targets, unsaved import removal/rebinding/restoration, references and an actually
applied rename compared against all four complete buffers. Both servers acknowledge
shutdown and exit cleanly; harness/source/user-file hashes match. An earlier
unique-name fixture let five return-chain checks pass without establishing receiver
inference. The reported comparison adds same-named, differently typed receiver
decoys in both arms; the earlier artifacts are preserved separately.

Workspace tests pass **1,218 / 0 failed / 4 ignored** across 40 suites. Formatting
and release build exit zero. The `library` editor probe passes **13/13**. Wider
editor diagnostics remain open: `features` returns **78/80**, and `java` **191/196**.
The previous release also misses the same two feature expectations and five Java
diagnostics; its feature run additionally returns a nil implementation result.
That extra observation is retained without attributing an unrelated improvement to
this patch. Missing-type explanation reports `compiler-producer-incomplete` and
`unknown`; expectations were not weakened or marked passed. The Java probe's
forced client termination is recorded separately from its diagnostic coverage gaps.
See `semantic-{before,after}-decoy/`, `editor-before-regression-results.json`,
`editor-regression-results.json`, `validation-results.json` and `slice-evidence.json`.
No latency, process-RSS or six-project IDEA-equivalence claim follows from these
fixture results.

Next reduce the compiler-valid function-local extension in
`compiler-project/src/main/kotlin/app/LocalUse.kt`, using
`local-extension-next.json` through the existing `corpus` harness. Local extensions
must retain lexical scope and typed receiver selection rather than enter the global
index. Kotlin's obsolete binary-member diagnostic still needs a complete visible
extension world and compiler-producer proof. Broad six-project IDEA comparison,
compiler-plugin editing behavior and matched performance/memory measurements remain
open. Execution timeouts bound experiments and do not decide semantic acceptance.

### Thirty-third checkpoint: function-local Kotlin extensions

Compiler-valid local extensions were intentionally absent from the cross-file index,
but semantic queries had no scoped replacement. The core now extracts their authored
definitions once per file context and carries shared immutable facts through Salsa.
Candidates retain declaration order and block ranges; real members precede applicable
inner local extensions, outer scopes and imported extensions. Typed overload selection,
generic receiver substitution and recursive calls use the existing member algorithms.
Local declarations never enter the global index or leak into sibling functions.

Navigation, inference, completion, diagnostics, references, rename, signature/hover,
type-definition and inlay hints receive these same facts. Local lambda receivers are
selected before spelling-based standard-library shortcuts; an inapplicable local
`run` still permits the actual standard-library function. Safe navigation selects
against a non-null receiver and preserves a nullable result. Unresolved receiver
relationships, written generic bounds and parameterized receiver constraints retain
Unknown rather than inventing a type or a target. Valid constrained-receiver cases
therefore remain explicit coverage gaps.

Evidence root: `C:/Users/gummy/AppData/Local/Temp/ktlsp-kotlin-local-extensions-20261011-1`.
The original immutable release SHA-256 is
`9e318a840222c184c2cea3c89bc0103615c6d9947fbeaed5ebb21e061751b6fc`; the fixed release is
`696ed0332e75346a4f67ce8fcaebcb6be74d2a1a0ac824be828fa7afe906c0e2`.
The same `semantic` scenario improves local checks from **31/44 to 44/44**, with
all **21 first responses** delivered without retries. Existing **23 ordinary and
42 alias checks** pass in both arms. Unsaved declaration rename, removal, return-type
rebinding and restoration include useful paired sibling/Java controls. The portable
source is `dev/semantic-fixture/probe/extensions/app/LocalUse.kt`; the report is
`semantic-local-extension.json`. Both servers acknowledge shutdown and exit cleanly;
source, harness and user-file guards match.

The unchanged three-case corpus manifest issues six completion/definition requests.
After indexing, the original release matches **2/6**, the fixed release **6/6** and
IntelliJ LSP **6/6**, all on their first response. IDEA imports the same disposable
Gradle project and uses JDK 17; its plugin is pinned to
`9cfad18005470e4187090013e08f92ec9e437219`. Its separate shutdown wait expires after
the six useful responses; that does not invalidate the observed semantic results.
See `editor-{before,after}-ready/` and `editor-intellij/`.

Gradle/KGP 8.10.2/1.9.23 and 9.6.1/2.4.0 agree on fourteen scope/overload controls.
Additional actual compiler runs cover the portable fixture, six complete renamed
variants, lambda receivers and nullable results: **53 expected outcomes** match,
including deliberately invalid programs. `compiler-validation-summary.json` links
their commands, source hashes and diagnostic output. Focused Rust verification passes
**31 tests across eight suites**. References distinguish nested/sibling scopes and
typed overloads; rename applies and compares all four complete buffers, then resolves
the edited uses again. Workspace tests pass **1,248 / 0 failed / 4 ignored**;
formatting and release build exit zero. See `rust-green-results.json`,
`validation-results.json`, `source-guards.json` and `binary-receipt.json`.
The wider `library` probe remains **13/13**; `features` remains **78/80** and Java
**191/196**, retaining the same earlier missing-type/absence diagnostic gaps. These
are unresolved analysis expectations, rather than failed source-availability gates.
The Java client's separate forced termination is retained in the operational record.
See `editor-regression-results.json` and `regression-{features,library,java}/`.

Next replay the existing Gradle and Spring incomplete manifests on this release,
then exercise a real Retrofit Java-to-Kotlin unsaved type change and applied rename.
The Kotlin obsolete-binary diagnostic, compiler-transforming editing cases, wider
constraint inference and matched six-project IDEA latency/memory measurements remain
open. Experiment deadlines and cleanup remain separate from analysis acceptance.

### Thirty-fourth checkpoint: separate analysis from experiment completion

The corpus harness now records three independent sections: `semantic`, `operations`
and dependency-source `coverage`. Each request retains `matched`, `mismatch`,
`unanswered`, `protocol-error` or `not-run`. Only valid RPC responses evaluate semantic
expectations. A valid empty positive response remains a functional mismatch; deadline
and transport errors leave the assertion unmeasured. Planned requests are registered
before initialization, so an interrupted import cannot become a zero-case pass.
Unknown source counts are JSON null rather than invented zeroes. Missing dependency
sources are input metadata, and required navigation cases retain their exact targets.

Unsaved probes require confirmed outbound edits before dependent requests. Failed
delivery leaves those requests unstarted; restoration is still attempted. If restoration
cannot be delivered, subsequent probes stop with an incomplete experiment. External
corpus hooks share the same outcome ledger. No request predicate, retry policy or
semantic expectation was weakened.

Evidence: `C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-outcomes-20261012-1`.
The headless matcher passes **44 checks**, including absent/error/empty/useful responses
and reviewed negative controls. The unchanged three-case local-extension corpus records
**2 matched / 4 mismatched** on the original release and **6 matched / 0 mismatched**
on the fixed release; all twelve requests use one valid first response. Their operational
records are complete. An explicit one-millisecond readiness observation control executes
**zero requests** and records **six not-run**, semantic **not-measured**, experiment
**incomplete** and unknown source counts. It supplies no product speed or correctness
conclusion. See `native-results.json`, `native-guards.json` and the three run directories.

This slice changes only Lua validation and documentation. The checkpoint-33 Rust and
release artifacts remain unchanged; no new Rust behavior or IDEA parity is claimed.
Next run the existing real Gradle/Spring incomplete manifests, then the real Retrofit
unsaved Java-to-Kotlin type change and applied rename with compiler/IDEA controls.

### Thirty-fifth checkpoint: stream large Gradle project models

The current Gradle corpus exposed two project-import defects. Its Kotlin DSL treats
warnings as errors: the embedded init script had a shadowed local name and deprecated
observer callbacks. A local rename and two scoped suppressions preserve those callbacks
without changing the project's warning policy. After that fix, an independent native
Gradle invocation succeeds and emits **112,887,895 bytes**, with **194 PROJECT / 194 END**
records. The server still discarded the model at its **64 MiB** stdout capture limit.
The resulting five required completions returned valid empty responses; this is a
functional import failure, not a timeout-derived analysis score.

Classpath discovery now drains stdout into a unique temporary file and parses it with
a buffered reader. It retains the existing Windows process ownership and EOF checks;
read, write, flush, native-exit and protocol failures cannot publish a partial model.
Strict discovery rejects nested, mismatched and unfinished project blocks. The public
partial-text parser keeps its existing best-effort behavior. The memory cap is removed
from model transport; ordinary compiler diagnostic streams remain bounded.

Focused validation passes **4 output**, **36 classpath** and **7 native ownership** tests,
including a complete **65 MiB** model with a tail module and failed/truncated controls.
The workspace passes **1,254 tests, zero failures, four ignored**, formatting passes,
and the release build succeeds. The immutable release is
`ktlsp-gradle-stream-20261012-1/bin/ktlsp.exe`, SHA256
`0369a6dd328615056ff34929f904ade03af41de5df472db175df7f51f47f60a1`.
The same real Gradle project now matches **6/6 incomplete** and **3/3 golden** requests
on their first valid responses, without retries. Its partial member, dangling dot,
broken peer and paired known/unknown receiver cases satisfy **13 semantic checks**.
Java-to-Kotlin constructor completion/navigation and Kotlin-to-Java interface
navigation satisfy the three golden checks. Edit delivery, restoration and shutdown
are recorded separately and complete. The last scan reports 11,325 project files and
57,686 library files; ten failed and twenty-five missing dependency sources remain
input metadata. Required targets still resolve exactly.

Evidence is under `C:/Users/gummy/AppData/Local/Temp/ktlsp-gradle-stream-20261012-1`
(`source-freeze.json`, `native-build-results.json`, `binary-receipt.json`,
`native-results.json` and both `gradle-stream-*/artifacts/corpus-results.json`), with the
independent native dump and its receipt in `ktlsp-gradle-init-warnings-20261012-1`.
Temporary disk usage grows with output; the parsed model and its longest record still
occupy memory. Unix descendant ownership remains unverified, and the current protocol
cannot detect omitted whole modules after a complete block. This slice does not claim
whole-project performance or IDEA parity. Next repair the independently reproduced
Retrofit Java generic/class-literal/extension-property inference gaps, then compare
useful native completion and memory under matched conditions.

### Thirty-sixth checkpoint: useful Moshi native completion and memory

The pinned real Moshi root uses the same Neovim 0.12.5 corpus/UI route, source and
JDK 17 for both engines. The official IntelliJ plugin remains pinned to
`9cfad18005470e4187090013e08f92ec9e437219`, server `263.6379.0`. Each arm starts
with a fresh per-arm cache, runs its three golden requests, then starts a new UI
server using that seeded cache. The Gradle dependency cache is already populated;
the arms run sequentially. These are editing measurements, not cold-start timings.

Both engines match **3/3 golden first responses** and **31/31 native completion
cycles**: useful LSP popup, one completion response, keyboard acceptance, exact
whole-buffer SHA256 and unchanged disk source. The first cycle and thirty warm
repetitions are separate. No empty or failed completion contributes a timing sample.

| Measurement | ktlsp | IntelliJ LSP |
| --- | --- | --- |
| First input to popup handoff (n=1) | 50.816 ms | 2,668.160 ms |
| Warm completion RPC p50 / p95 (n=30) | 11.003 / 12.893 ms | 421.040 / 723.694 ms |
| Warm input to popup handoff p50 / p95 (n=30) | 43.927 / 48.219 ms | 453.308 / 763.453 ms |
| Warm acceptance to verified text p50 / p95 (n=30) | 126.014 / 127.859 ms | 105.863 / 134.707 ms |
| Working-set median during first post-readiness second | 854.0 MiB | 1,892.0 MiB |
| Process peak working set through last observed sample | 856.1 MiB | 2,760.7 MiB |

The PID marker is captured from the actual synchronous `vim.system` spawn. External
observation verifies the executable, creation time, direct editor parent and ancestry
to the owned harness. IntelliJ's same PID has its bundled `jbr/bin/server/jvm.dll`
loaded, so its memory observation includes the embedded JVM. The pre-existing user
Java process is untouched. Memory excludes Neovim/Gradle child processes; the first
second includes editing and is not idle steady-state. Peak scope ends at the last
observed sample. Popup handoff is not physical-screen latency. This single case
establishes neither whole-project semantic parity nor a universal performance budget.

The separate IntelliJ corpus shutdown observation expires after useful responses;
all UI samples remain valid and the native artifacts retain operational exit 1.
The subsequent UI server exits cleanly. The earlier corpus process's exit was not
observed, so residual background work is not independently excluded from the timing
comparison; repeat with that operational control before claiming a stable speed ratio.
Evidence: `C:/Users/gummy/AppData/Local/Temp/ktlsp-moshi-ui-matched-20261012-1/`
(`native-results.json`, both `artifacts/completion-ui.json`, `source-guards.json`,
`fixture-publication-receipt.json`). The committed `dev/corpus/moshi-1.15.2-ui.json`
is byte-identical to the tested manifest. The server is checkpoint 35's immutable
release. Other project UI/memory comparisons and the real Retrofit generic chain
remain open; the latter is independently reproduced with compiler controls.

### Thirty-seventh checkpoint: actual Retrofit unsaved generic baseline

The new `dev/nvim_retrofit_mixed_probes.lua` uses the existing corpus hook on pinned
Retrofit with its main JDK 8 toolchain and Gradle running on JDK 17. Four original
files remain unchanged on disk. It changes Java `Invocation.service()` from
`Class<?>` to `String` in memory, then checks Kotlin's members and exact definitions.
It restores the type, checks Kotlin/Java references, applies `service` to `serviceType`
across four complete buffers and inspects fresh navigation/references after rename
and restoration. A failed prepare-rename leaves dependent operations unmeasured.

Independent compiler controls for original, typed and renamed snapshots and two
deliberately invalid member calls match all five expected outcomes. The narrower
tag reduction also compiles with actual Gradle/KGP 1.9.23/JDK 8; two wrong-class
variants fail at `service` as expected. It separates invariant `Class<T>`, bounded
Java `Class<? extends T>`, named Kotlin class literals and `KClass<T>.java`.

On checkpoint 35's immutable release, the actual Retrofit run has **18 valid first
RPC responses: six matched and twelve mismatched**. Three rename-dependent requests
are not run, and four applied-buffer assertions are consequently unmeasured. These
are functional empty/missing-reference results, not deadline or source-count scores.
The small reduction's ordinary receiver and invariant generic control match four
requests; its other nine requests return valid incorrect/empty results. All seventeen
reduction requests, including the paired negative cases, run without protocol errors
and with zero failed or missing sources. Wrong-member exclusions alone do not pass
when the required positive `body` completion is absent.

The same actual Retrofit hook through the pinned IntelliJ plugin matches **21/21
first RPC responses and 25/25 semantic checks**, including all four renamed complete
buffers. This fresh native run exits cleanly. The oracle admits two source-reviewed
representations: one-argument substring at JDK 8 or its pinned Kotlin stdlib wrapper,
and exact Java identifier or exact `invocation.service` member-expression reference
ranges. Wrong overloads, source versions, extra targets, parentheses, duplicate sites
and broadened Kotlin ranges remain rejected. **64 native matcher checks** pass;
replaying the earlier payloads accepts all five legitimate IDEA alternatives while
retaining four ktlsp mismatches and one unrun request.

Evidence: `C:/Users/gummy/AppData/Local/Temp/ktlsp-retrofit-mixed-oracle-20261012-1/`
(`native-results.json`, `source-guards.json`, both `artifacts/retrofit-mixed-requests.json`
and snapshots); independent compiler controls are under
`ktlsp-retrofit-mixed-unsaved-20261012-1/compiler-results.json` and
`ktlsp-retrofit-tag-reduction-20261012-1/{native-compiler,native-before,native-oracle}`.
Next repair named class literals and declaration-scoped extension-property bindings
in shared facts, then Java wildcard constraint inference. No Kotlin completion or
rename parity is claimed from the old six passing golden requests.

### Thirty-eighth checkpoint: ordinary editor observation boundaries

The existing `features` and `java` probes now retain each RPC's raw response/error,
attempt count and observation interval. No response, transport error or unstarted
request is unmeasured and still causes a nonzero harness exit. A valid empty or wrong
RPC result fails its original positive expectation; negative assertions cannot pass
merely because no response arrived. Deadlines bound observation, not speed acceptance.

Diagnostic observation starts before the edit and records outbound document version,
changedtick and complete source hash. It inspects the first target notification,
retains rejected notifications and rejects intervening edits or another outbound
change. Exact versions are checked when supplied. A versionless notification records
the limited assumption that the first target notification after the outbound change
belongs to that unchanged buffer. It does not certify the server's analysis generation
or final compiler pass; a future fast/compiled two-phase stream needs an independent
accepted-store completion barrier before final diagnostic expectations are judged.

Native Neovim matcher controls pass **123 checks**, fourteen existing lifecycle
controls pass and all four changed Lua files parse. Against immutable candidate
`c129e119211c142588dc53bc604a55408a6315cbddac455bdd3c317f15821ce0`,
the ordinary features probe retains **78 matched / 2 unmet checks**, including
**43 matched / 2 unmet semantic checks**. Java retains **194 matched / 5 unmet
checks**, including **55 matched / 5 unmet semantic checks**. Counts also contain
setup/assertion controls and are not project acceptance percentages. There are no
unanswered semantic observations in these runs. All seven prior positive demands
remain: missing type/resolution, missing Java call, arity/type errors and two pairs
of invalid calls after changing a superclass. The six relevant push observations
are versionless and generation-uncertified. `explainResolution` still returns a
valid Unknown under compiler-producer incompleteness. These coverage gaps remain open.

Both scripts explicitly request forced stop; the observed exit 1/signal 15 is kept
as an operational record, not another semantic failure. The sample's disk hashes
remain unchanged. Evidence is under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-harness-acceptance-native-20261012-1/`
(`results.json`, `editor-results.json`, `sample-source-guard.json` and each scenario's
structured result). The candidate has 1,270 passing Rust tests, four ignored and a
successful release build; its real stdlib generic-chain repair is still in progress.
Next verify the shared declaration-shape repair on the unchanged real-library reduction,
then establish same-generation canonical compiler diagnostics without relaxing these demands.

### Thirty-ninth checkpoint: typed extension receivers and borrowed symbol queries

Shared Kotlin facts now retain a declaration-scoped, parameterized extension receiver
and inline upper bounds. Completion, member selection and result inference use the
same receiver proof. Named non-generic class literals produce `KClass<ExactType>`;
invariant arguments, repeated variables, nullability, declaration shadows and
conflicting/incomplete hierarchies retain their distinctions. Kotlin block-bodied
functions without a return annotation have intrinsic `kotlin.Unit`. Unknown arguments
and unsupported constraints do not become precise matches. The additional facts are
boxed only for extensions. Both dependency-symbol and ktcheck durable-cache versions
invalidate old positional encoding. The existing CLI warm-cache test now round-trips
an extension receiver and bound and verifies their restored shared file context.

The real Kotlin 1.9.23 source archive contains common `expect KClass<T>` and JVM
`actual KClass<T>` declarations with different parent lists. Nominal identity/arity
proof now requires agreement on package, container, kind and ordered type parameters;
parent traversal still requires agreement on the instantiated hierarchy. This lets
`KClass<T>.java` and `Class<T>.cast` retain useful types without pretending conflicting
parent lists prove a subtype relation. Conflicting kind/arity/type-parameter controls
remain Unknown. This is not a general expect/actual merger.

On the unchanged real-library tag reduction, first-response RPC matches improve
**4/13 to 9/13**. The broader thirteen-probe manifest has **13/23 matched RPCs**;
all ten remaining valid mismatches pass through Java `Class<? extends T>`. Both paired
negative cases still lack their required useful known receiver, even though their
wrong-member exclusions match; they remain unmet. All requests run, with zero failed
or missing dependency sources and unchanged disk/manifest hashes. Independent selected
Gradle/KGP 1.9.23 compiler controls cover original and invalid tag calls, intrinsic
Unit, nominal bounds and nullable bounds. The full actual Retrofit unsaved/rename hook
has not yet been replayed on this slice; checkpoint 37's twelve gaps remain open there.

`workspace/symbol` now filters borrowed index entries, sorts only matches and clones
the final capped 200 summaries. It preserves Unicode lowercase/detail matching,
duplicates, complete payloads and the former tie order, including after remove/reinsert.
Three real Moshi arms each match 35/35 semantic checks; all 32 ordered symbol payloads
are identical, including 173 `JsonReader` results and the empty-query cap of 200.

| Sequential Moshi arm | First request (n=1) | Warm p50 / p95 (n=30) |
| --- | --- | --- |
| Published full-clone baseline | 1,446.964 ms | 1,530.612 / 1,775.314 ms |
| Filter before summary cloning | 911.732 ms | 965.891 / 1,127.982 ms |
| Borrowed entries before filtering/ranking | 512.040 ms | 500.953 / 528.668 ms |

These use the same frozen hook/project/JDK, fresh per-arm server caches and completed
indexing. Memory is unmeasured for these query arms. Sequential runs and coexisting
semantic changes do not establish a universal speed ratio or IDEA comparison.

Formatting, all **1,272 Rust tests (four ignored)** and release build pass. The real
Neovim basic/library probes pass 9/9 and 13/13; semantic retains 107 passing assertions.
Features remains 78 matched/2 unmet (43/2 semantic), Java 194/5 (55/5 semantic), with
the same seven original expectations and diagnostic-finality limits from checkpoint 38.
The Windows process-ownership tests now wait on the retained descendant handle after
asynchronous job termination; they still require observed termination and do not change
runtime behavior or impose a project-analysis deadline.

Immutable binary SHA256:
`7c68a7ff961470d73b41163093e9c2a23223c7bccea9ab4795841613198b74f0`.
Evidence is under `C:/Users/gummy/AppData/Local/Temp/`:
`ktlsp-retrofit-tag-reduction-20261012-1/{first-native-repair-7,native-first-shape-after,native-shape-editors}/`,
`ktlsp-retrofit-tag-reduction-20261012-1/first-cli-cache-repair-native/`,
`ktlsp-moshi-workspace-symbols-20261012-1/` and
`ktlsp-native-exit-handle-20261012-1/`. Exact commands, source hashes, raw responses,
timing samples and unchanged-source receipts are retained there. Next implement the
smallest shared Java projection constraint slice, replay the actual Retrofit hook,
and verify selected-compiler diagnostics for unsaved Kotlin through an accepted-store
completion barrier. The six-project goal remains in progress.

### Fortieth checkpoint: selected Kotlin compiler runtime recipes

Gradle metadata now captures the task's actual ordered compiler runtime separately
from its target-library classpath, optional tools JAR, effective target SDK, original
toolchain SDK, Gradle host JDK, daemon/BTA policy and opaque daemon JVM arguments.
The version comes from the selected runtime's isolated class loader, which also
verifies the JVM compiler entrypoint; the KGP plugin version is not substituted.
Capture is lazy inside the owner project's dump task action. A failed/incomplete
recipe retains source ownership and reports incomplete compiler metadata.

Project model version 7 invalidates earlier cached snapshots. The importer disables
Gradle configuration caching until replayed provider values are independently proved
fresh for compiler recipes and source membership. The existing live metadata probe
expects this current version; none of its source, classpath or toolchain expectations
is relaxed. This configured recipe does not prove that a daemon or build executed.

Actual candidate-script controls and the embedded release importer both capture
Kotlin 1.9.23 and 2.4.0. Recipes match independent probes in the same project, including
the exact ordered seven/fourteen runtime paths. The first uses Gradle JDK 17 with
target toolchain 8 and DAEMON/BTA-disabled policy; the second uses JDK 17 and
IN_PROCESS/BTA-enabled policy. Independent launches of the selected runtime report
the captured version. Runtime path order is preserved; it is not sorted or inferred
from dependency filenames.

Formatting, **37 classpath tests**, the full **1,273 passing Rust tests (four ignored)**
and release build pass. The settled first-response tag reduction retains **9/13 RPC
matches**, with its four known Java wildcard mismatches and no unanswered requests.
The automatic-watch Gradle lifecycle retains **85/86 original checks**, including all
ten metadata checks, four actual builds, dependency addition/version switch/removal,
generated sources, preserved unsaved buffers and clean shutdown. Its old Kotlin
obsolete-member diagnostic remains unmet; the compiler worker is still a separate
unmerged candidate.

Two initial observation mistakes are retained in the artifacts: the first tag run
omitted the existing indexing barrier and allowed repeated requests; it is not the
matched first-response comparison. The first 2.4.0 recipe comparison used another
disposable project's local Maven paths. Rechecking the same project matches exactly;
paths are never normalized away to manufacture equality.

Evidence: `C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1/`
(`candidate-owner-native/`, `native-embedded-recipe-1/`, `native-importer-1/` and
`native-importer-controls-2/`). Embedded release SHA256:
`b1adf7d0bc5a64f4b405c5fdcf161a37e8fea43ad33b3e3f2626decd31ae165e`.
Next verify the private unsaved-source compiler overlay, accepted-store generation
correlation and cancellation on actual 1.9.23/2.4.0, without replacing the original
missing-diagnostic expectation or declaring producer closure.

### Forty-first checkpoint: selected Kotlin diagnostics for unsaved sources

Supported captured Gradle tasks now run their selected ordered Kotlin compiler runtime
against private UTF-8 overlays of owned unsaved Java/Kotlin files, including new editor
buffers. Original source files and Gradle outputs are untouched. Accepted results enter
the same standard LSP push/pull diagnostic store; the fast diagnostic phase and
completion requests remain independent of the compiler process.

Acceptance checks captured source bodies, project-model identity, scope generation and
compiler-input identity. Edits, reloads and input changes invalidate stale work, including
A-to-B-to-A changes. Failed, cancelled, partial or unmapped compiler runs cannot publish
a fabricated empty result. A non-clean compiler pass may publish mapped positive errors;
clearing captured files requires a successful clean pass. Neither case establishes
general generated-source or compiler-plugin producer closure.

Compiler inputs are fingerprinted once off the interactive path and reused behind
precise native input watches. The watch plan covers the selected runtime, target libraries,
plugins, declared inputs and required host/target SDK parts; it does not watch every
workspace/JDK source file. Latest pending work replaces obsolete work in its scope,
shares the existing compiler semaphore and uses owned native process jobs. Closed scopes
retire their cached input environment. This worker is currently verified on Windows;
other hosts retain an explicit unavailable result.

Formatting, **67 focused Kotlin tests**, the full **1,285 passing Rust tests (four
ignored)** and release build pass. The prior worker verification also retains **13
compiled-watch tests** and **seven native-ownership tests (one ignored)**. Its minimal
module-name repair preserves Gradle's original name containing a colon; the selected
2.4 compiler handles that name itself.

The actual native Neovim automatic-watch Gradle lifecycle passes **86/86 unchanged
checks**, including the formerly missing Kotlin obsolete-member error. It performs four
real builds, dependency addition/version switch/removal, generated-source changes,
preserved dirty buffers and clean shutdown. Observation is armed before one edit. The
accepted receipt hashes the actual transmitted CRLF full text, and one standard pull
returns its positive error with the same analysis/source/store generation. A versionless
push proves delivery only; the accepted receipt and correlated pull prove this result.

A separate actual **Kotlin 1.9.23** run opens a new Kotlin buffer without creating its
source file on disk. It reports the expected unresolved call at the exact UTF-16 position
after Japanese text and an emoji. A second unsaved edit fixes that call, and a clean
selected-compiler pass clears its error. Both canonical observations match and the
original project/source/build files retain their hashes. The observed diagnostic waits
are **3,573.9 ms and 3,949.3 ms, one sample each**; these include queued compiler work and
pull observation. They are not completion latency, p50/p95, a performance gate or a
memory measurement. Wider worker editing latency and RSS still require measurement.

The existing editor observer now preserves LF, CRLF, CR and the final-newline setting
when hashing full-text edits, and checks the actual outbound payload. **326 matcher
controls**, **14 lifecycle controls** and parsing of **seven Lua files** pass. It waits
for a content-independent accepted-generation receipt, never for desired diagnostic
content. A missing receipt is unmeasured; a certified wrong/empty positive response is
a semantic mismatch. The original diagnostic expectation is unchanged.

Initial attempts are retained separately: the worker rejected a colon in the task's
module name; the observer hashed LF while Neovim sent CRLF; a disposable 1.9 hook passed
a table to a string-only logger after its positive assertion had already matched. These
are distinct implementation/observation errors, not final passing proof.

Evidence: `C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1/`
(`native-worker-module3/`, `native-worker-canonical5/`,
`native-worker-selected-1.9-7/` and `root-harness-wire-apply2/`).
Immutable release SHA256:
`e855d08ecb1b4ce5e6c0c134c25e75ce32ecb952e1b3845331d855c78cd0e4d0`.

Opaque plugin options, arbitrary argument files, unmodeled flags/source filtering and
dirty visible sources from other source sets remain unsupported. The same seven ordinary
non-Gradle diagnostic demands remain open. Next verify the shared Java/Kotlin wildcard
constraint slice against real compiler and unchanged Retrofit editing probes, then
measure wider worker editing latency/RSS and compiler-plugin behavior. The six-project
goal remains in progress.

### Current slice: requires-only compatibility artifacts

Selected Kotlin compatibility and coroutines JARs can contain a module descriptor
and no ordinary declarations. Requiring nonexistent source definitions for those
artifacts left otherwise useful indexing incomplete. The shared classfile reader
now proves a narrow requires-only module shape, and archive classification checks
the actual selected binary before treating it as having no ordinary API. Unknown
versions, attributes, pool tags, additional definitions or malformed input remain
inconclusive. Read/CRC failures remain errors; work is bounded to 50,000 entries
and one 16 MiB descriptor. The existing archive reader still loads the compressed
archive in full.

The portable fixture set contains 26 classfiles and five original small Maven
JARs, with exact byte/SHA manifests, source/compiler provenance and licensing.
Five actual selected artifacts fail before repair; thirty-six distinct focused
checks pass afterward, including authority, corrupt/missing archives and worker
coverage controls. The accepted eleventh full/release checkpoint includes this
slice. Evidence is `ktlsp-module-only-20261011/implementation/final-proof.json`.
The separate external Rust consumer now builds with `--release --locked --offline`
and passes **140/140** unchanged checks through the real Neovim corpus route,
native editor exit zero. Its five features, 29 first-response requests, nineteen
delivery barriers and four applied/inspected renames pass with zero failed/missing
inputs. The earlier 139/140 failure remains preserved. All 306 product, four
consumer, 27 fixture files and strict probes match before/after. Evidence is
`ktlsp-shadow-eleventh-fixed-1-20261011/provider-evidence.json`; the directory name
is retained, but its guarded product snapshot is eleventh-fixed-2. The distinct
consumer binary SHA-256 is
`34a4a0720fd7f6b3cc8c0f15e1e23fdec97e0c8d6aa555ceaeca1c81e934c187`.
Its legacy client shutdown assertion is not used as independent process-exit proof.
The large Gradle replay separately reduces missing inputs from thirty to twenty-five.
This is no general binary-source replacement or compiler-producer closure proof;
remaining real-root coverage and generated APIs still need work.

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
