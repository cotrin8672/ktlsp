# Current fork state

Updated 2026-10-26 (Asia/Tokyo). Goal active. General IDEA parity and language-wide
correctness coverage are unmeasured; passing selected probes does not establish either.
User priority is valid-code Unknown/missing completion and false diagnostics, followed
by editing UX. Warm speed is accepted provisionally; cold startup is lower priority.
KAPT/KSP execution belongs to the external build, not to ktlsp's analysis. The
user explicitly rejected LSP-owned regeneration. Remove the private generation
executor and analysis-triggered Gradle replay; do not retain them as optional
features. Existing generated sources/classes, dependencies and plugin configuration
must instead be reflected in editing features and refreshed after an external build.
The [official KSP quickstart](https://kotlinlang.org/docs/ksp-quickstart.html#use-your-processor)
explicitly says that an undefined generated function in the IDE is expected before
building/running. This does not waive ordinary unsaved Java/Kotlin semantics or
virtual-member support supplied by compiler plugins.

Evidence paths below are relative to RUN:
`C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda`.
The complete previous record is preserved in
[history through slice139](fork-history-through-139.md). Older acceptance matrices
are historical observations, not a list of current regressions. Performance targets
and comparison rules remain in [performance budgets](fork-performance-budgets.md).

## Current verification: candidates56 through58

Candidate56 passes the complete Rust workspace: **1845 passed, 0 failed,
13 ignored**, followed by a successful release build. The immutable binary is
22,779,904 bytes, SHA256
`83450102d81324bce53fa8c7beb8986d405afbd20fa53ad6ca227e79c843944a`
(`native-integrated120-56/main-ready.json`). Both stages reach EOF/live0;
one exact owned MSVC telemetry helper is retired in each stage. This is a full
suite result; candidate55h's earlier repaired-test composite remains separate.

Actual Neovim on56 passes all original **CEM38/38**: normal9, incomplete17,
getter8 and TypeTools4. All four stages have stable source/runtime guards,
clean shutdown, EOF/live0/forced0 and a restored dedicated Gradle profile
(`cem-final153-candidate56-1/adjudication-review155.json`). Missing source
archives2 remain a coverage observation. Candidate55's CEM failures are retained:
its Gradle9.6.1 init script failed Kotlin type inference in an unused action
return. Removing that return fixes import; the original identity/resource and
configuration-cache controls also pass on real Gradle9.6.1
(`observed-action156-g9/receipt.json`).

The dependency is ordinary registry **zip9.1.1**, with its checksum in Cargo.lock;
there is no new fork, submodule or local ZIP patch. Its upstream Unicode-extra
fix admits the actual JNA archive. Payload CRC and understated-size rejection
remain strict. Focused JNA/CRC/size controls and the complete56 suite pass.

Candidate57s subsequently passes the full Rust workspace **1852/0/13** and the
release build (`native-integrated120-57s/main-ready.json`). Actual external
Java/Lombok now passes **75/75** unchanged checks: initial `getName`, fluent
configuration, field rename to `getTitle`, restoration, stale API withdrawal,
String hover/chain completion and preserved unsaved consumer text. All four
external Gradle builds, native watches and first responses are observed;
EOF/live0/forced0 and stable guards hold (`j57sd/receipt.json`). A separate
disposable online bootstrap populates a dedicated real Gradle home; the actual
offline test starts without class outputs. No LSP generation command is used.
The fix readmits current external facts after source/POM model enrichment;
previously that model replacement cleared the freshly installed getter.

Remaining current work is explicit: Moshi9, OkHttp10 and Retrofit12 selected
checks pass on56; Spring and the Gradle counter are still in dependency indexing.
The ordinary dirty-Java dependency header route passes focused checks but its
original current-version Gradle counter is not yet proved.
Candidate58 passes the complete workspace **1857 passed, 0 failed, 14 ignored**
and both release builds. Its retained native K1 check separately verifies the
new Kotlin-backed Java error and its current Workspace publication; actual raw
Javac still reports all three expected errors. The unchanged original external
KSP/KAPT workflow now passes **114/114** in real Neovim: its first Java pull
returns all three located errors, including the previously omitted generated
Kotlin method. Four external builds return0,0,1(expected disabled),0, with current
wire/canonical results, stable guards, clean shutdown, EOF/live0/forced0 and
restored dedicated Gradle settings (`processor120/p58/adjudication-review155.json`).
The historical55h113/114 result remains preserved.

The mc-dev source-inventory/current-source-snapshot API now passes **158/158**
real Neovim/child checks:35 legacy/common and123 source/project checks, all
measured and matched. Both inventories contain9 sources with complete coverage;
five closed-file bodies match independently read disk bytes. Unsaved Unicode
text, identity/revision/SHA, empty Java text, model/stale rejection, crash/restart,
applied rename and disable all pass. Four owned stages reach EOF/live0/forced0,
restore build files and preserve the dedicated Gradle profile. Independent
recalculation agrees (`source-snapshots155-58-independent-review.json`). The
historical57s126/128 result and its two closed-source failures remain preserved.

Source bodies are prepared outside Workspace locks from accepted project inputs;
installation checks the model, source hashes/stamps and current unsaved text.
The existing evaluation lifetime tracks membership, body and model changes.
Inventory paging preserves opaque handles and byte/batch limits. Two actual
Gradle source sets have1257/1276 files and exceed512KiB as whole inventories;
the handler/serializer and client batching tests pass, and measured real text
fits the existing limits in27/31 whole-client batches. This is not a complete
Neovim replay of those large source sets. Resources, raw class bytes/Code/LVT
and command routing remain separate contracts.

The remaining indexing bottleneck is now localized with a debug replay of733
real Spring source files and four dependency sources: index work takes roughly
4–47ms while repeated publication/normalization takes6.85–14.08s. One bounded
four-file publication takes1.51–2.77s. This identifies repeated flush work;
it is not a release latency or user-experience speedup claim. The temporary
measurement code is removed and the production improvement is in progress.

## Previous checkpoints: releases42 through54

The private-generation results below document a superseded design, not additional
completion requirements. The source now removes that executor, its cache and
diagnostic authority, and automatic Gradle archive replay from analysis. Candidate53
passes six focused controls. Candidate54 passes three further controls, including
known KAPT/KSP plugin rejection before JVM launch and the restored actual Javac
generated-API/Unicode-range check. Release54 passes1840 tests,0 failures,13 ignored;
the full release build succeeds with605 compiled inputs unchanged. Its immutable
binary is22,526,464 bytes, SHA256
`8d83259301e36b628ac4ee404fa60cddb740d9b6b86c390e2cc73aa39cd1d617`
(`native-integrated120-54/main-ready.json`). Both owned stages reach EOF/live0;
each retires one exact owned MSVC helper, with no JVM/server involved.
Generated-input analysis and update checks remain required; they use actual external
builds rather than LSP-triggered generation.

Actual54 external Java/Lombok ingestion reproduces a product gap: the external
Gradle build succeeds, current class hashes and the latest model/index generation
are observed, and authored completion works, but the first generated `getName`
completion is empty. There are17 matched checks and1 mismatch; later configuration,
source-change and restoration phases are not run. EOF/live0/forced0, stable guards
and the restored H17 profile are verified (`external-java155-release54/receipt.json`).
This is generated-output analysis still needing a fix, not a need for LSP generation.
The separate KSP/KAPT114 run first lacked the offline Kotlin1.8.21 plugin marker;
genuine external Gradle resolution now succeeds. An overlong Windows service-file
path is also retained as a setup failure with zero LSP requests. The original114
checks are running from a shorter disposable path; no source/model cache is fabricated.

The compiler-task observer fix is committed and pushed in
`8b0f9129fed8f5b24b8e340d8fe7a5a1e85a8cba`. Actual Gradle8.10.2 cold/warm controls
verify that only exact owned action wrappers are excluded, user actions remain
visible, and configuration-cache reuse succeeds (`observed-action155-frozen1.json`).
This does not itself establish generated-code ingestion in an editor.

Release50 passes **1869 passed, 0 failed, 15 ignored** and builds successfully.
All615 inputs remain unchanged; both stages reach EOF/live0/forced0. The immutable
binary is23,269,376 bytes, SHA256
`1b8821fab0f26d33f1c11f5651ddb0f8bfb5d4d727375abff136dae0a7881663`
(`native-integrated120-50/main-ready.json`). The source guard packet binds32
selected inputs. These candidates also contain unpublished working-tree analysis
and processor changes; their binary/source receipts distinguish them from Git HEAD.

The four-task KAPT executor now has focused contract tests and an independent
review covering current generated outputs, three normal producer modules/six
output slots, temporal reads, cache validity and retained private-generation
ownership. This is still not proof that the actual OkHttp KAPT run succeeds.
The direct51 native run now completes Gradle successfully, but current-result
acceptance rejects `:okhttp:copyJavaTemplates` reading its authored template outside
the modeled source closure. This is a real unsupported input role, not a timeout.
Candidate52 models that precise role and checks the current template, expansion
configuration and generated contents. Its three focused tests pass. The same
native control completes Gradle successfully but rejects an external input that
was not prearmed; the exact offending read is not established. Current native
receipts are retained before that rejection. No current Java-body success has
been established for this workflow, and further unsaved-KAPT closure expansion is
deferred (`diagnostic155-kapt-direct52/receipt.json`,
`build155-candidate52-native-red-handoff.json`). A separate review found that the
template cache lacked a current expansion-configuration binding; its safety fix
is pending, so template-cache reuse is not an accepted workflow.

An unchanged observed-Java plugin probe on release50 also reproduces a missing
generated getter. Native `:compileJava` succeeds, but configured-task equality
rejects the result: the only configuration difference is two action wrappers around
the same stock action. Their exact ownership is under investigation; user actions
and other configuration changes must remain checked. Authored completion and goto
pass; the generated getter fails, and later configuration/dirty phases do not run
(`observed-java155-release50-2/adjudication.json`). Its genuine online Gradle/Lombok
bootstrap succeeds; the semantic probe itself remains offline and unchanged.

The durable Java-only corpus probe is committed and pushed in
`b4ef819072e435f63b9394d620e01950f5e2f178`. Actual48 verifies four first-response
RPCs/eleven semantic checks, v3 error then v4 repair, exact wire/result identities
and raw Javac input/body-receipt joins, with restored buffers/profiles and
EOF/live0/forced0 (`java-body155-release48/acceptance.json`). ROOT rehashes all
eight referenced artifacts and independently recomputes both canonical raw-receipt
digests. This remains one K2 mixed module; it does not establish KAPT/K1/producer
closure. The existing corpus route and library generator reproduce it.

Gradle48 still has no currentv4 counter receipt. This run has no model-refresh
events, but does receive directory-only resource-watch notifications. Resource
observation invalidates the current ticket, while the directory has no project
refresh and the old handler returns without requeuing diagnostics. Release50
reschedules selected diagnostics after accepted nonrefresh observations. Its real
Backend regression covers ordered Java/Kotlin edits, both current wire snapshots,
the same analysis generation/model, deduplication, and rejection of an otherwise
publishable stale result. Actual50 still has no current counter receipt. It does
exercise coalescing into the already queued generation. Both obsolete workers join
in about9 seconds; the subsequent retirement gap is not attributed to that preview.
Candidate51 also makes post-index archive validation cancellable and rejects its
cancelled result at the outer acceptance boundary. Its focused tests pass; the
actual Gradle counter still needs verification. No full17 replay is started from
earlier unmeasured selective results (`gradle-counter155-selective50/tail-retirement-supplement.json`).

Release47 passes **1865 passed, 0 failed, 14 ignored** and builds successfully.
All613 input guards, including the new producer helper, remain unchanged;
both stages reach EOF/live0 and each cleans one exact-owned MSVC helper.
The immutable binary is23,198,208 bytes, SHA256
`812cdc361fb69ad029ad6e29580a35d1888d428411f1faccef288577532cbad3`
(`native-integrated120-47/main-ready.json`). It includes the current normal-output
closure for three Kotlin producers, the exact captured Gradle7.5 Java19-option
variant, and cooperative cancellation of active SDK/JDK/binary indexing. Focused
tests cover both Java option variants, six output bindings, stale/cyclic inputs,
owned-worker cancellation, stopping JDK archive reads, and partial-cache rejection.
Actual47 OkHttp Java error/repair/cache checks and the Gradle counter still do not
establish current diagnostic acceptance. Both runs shut down cleanly, restore
buffers/profiles, and retain their unmeasured cases rather than counting a timeout
as either semantic correctness or a false diagnostic.

OkHttp47 again matches the original16 operations, but its added Java error control
has no current compiler receipt; repair/cache/API follow-ups are not run
(`okhttp-java155-release47/adjudication.json`). A direct captured-model Rust check
now admits all three regular producers and identifies the consumer boundary:
guide has an active20-entry Moshi KAPT path and four compiler tasks. It cannot be
treated as an inactive processor. The existing private processor execution supports
a fixed five-task KSP/KAPT chain; supporting the actual four-task chain must also
include the three regular producers and current generated outputs. That work is
under implementation, with no Java-clean authority granted by merely changing a
predicate (`diagnostic155-admission48/bounded-admission-result.json`).

Gradle47 reaches the originalv4 counter but still has no accepted current receipt
within the unchanged observation. The obsolete227-second preview is not observed:
after a cancellation, the next model/source install and binary preview progress.
Further build-output notifications then request generations5 through8, and the
target remains unmeasured. Active cancellation is useful mechanism evidence, not
proof that the complete counter workflow is fixed. The owned run86867 terminates
cleanly with stable inputs and the original profile restored; no full17 replay is
started from this failed selective proof.

Release46 passes1861 tests and builds, but its actual OkHttp run only verifies the
original16 operations. The added current-Java error check remains unmeasured;
repair/cache/API follow-ups were not run (`okhttp-java155-release46/adjudication.json`).
Captured Java19-option producer recipes were rejected by a parser accepting the
guide's21-option shape. Release47 adds that exact observed variant while retaining
processor, order, encoding and output-role checks. This still needs runtime proof.

The actual Gradle45 counter also remains unmeasured with its exact originalv4
wire hash and300-second observation. Two input inventories cancel on build-logic
JAR updates. Obsolete generation5 binary preview then takes227.611s, including a
213.631s SDK installation that publishes zero files, blocking generation6 behind
singleflight. The source-query cancellation had already completed. Full-trace and
source mapping are retained in
`gradle-counter155-selective45/causal-scheduling-audit.json`; the SDK subcost is not
attributed to lock waiting, disposal or computation without evidence. Release47's
cancellation fix passes focused tests; the actual editing result is still pending.

Release45 passes **1860 passed, 0 failed, 14 ignored** and builds successfully.
All611 input guards remain unchanged and both build stages reach EOF/live0;
each records one exact-owned MSVC helper cleanup. The immutable binary is
23,084,032 bytes, SHA256
`430c11893e76339eb79182e0e9bb3df2f7139512aaee8c73b4a37bca44a26256`
(`native-integrated120-45/main-ready.json`). Focused checks cover Java-only
buffer scheduling, separate current Java-body receipts, rejection of processor
fallback as dependency-closure authority, and scheduling before an optional
dependency-source query finishes. The actual Java-only editor check also passes:
four first-response RPCs/eight semantic checks, located Javac error at wirev3,
then a separate clean Java-body receipt at wirev4. Its raw body receipt SHA matches
the accepted certificate exactly, with stable18 guards and EOF/live0/forced0
(`java-body155-release45/acceptance.json`). This is one controlled K2 mixed module;
it does not establish K1 or external-producer closure. The actual Gradle counter
check remains in progress.

Release44 also passes **1857 passed, 0 failed, 14 ignored** and builds successfully.
Its610 input guards remain unchanged, both build stages reach EOF/live0, and each
records only one exact-owned MSVC helper cleanup. The immutable binary is
23,056,896 bytes, SHA256
`43a959df9bb9a1b3007e8991db192f017b81aaeef2dcc55be3916b65645456b8`
(`native-integrated120-44/main-ready.json`). Modeled output notifications now use
the existing owned-import coalescing path. The actual selective Gradle counter
still produces no current certificate within its unchanged300-second observation.
Output notifications settle, but an optional dependency-source query delays the
selected compiler queue (`gradle-counter155-selective44/selective-counter-adjudication.json`).
Release45 moves that queue earlier; its actual result is still unverified.

Release43 passes the workspace suite: **1857 passed, 0 failed, 14 ignored**.
The release build succeeds with609 unchanged input guards. Its immutable binary is
`native-integrated120-43/bin/ktlsp.exe` (23,053,312 bytes), SHA256
`2a8ace10d845b2a30d23260d86706d81cea3f0b779313a808eadbe73eeb46173`.
Both build stages reach EOF/live0. Each cleans one retained, exact-owned MSVC
`vctip.exe` helper; this is recorded separately from language-server termination.
Release42 also passes1856 tests and builds with zero forced cleanup. Its SHA256 is
`bccc86fd4139355d20ffaf2d3a197e3d7a2b9d159fa07ae78203370eb6061133`.

| Feature or workflow | Current actual evidence | Limit / next action |
|---|---|---|
| CEM excluding Mixin semantics | Release42: original38/38 matched, no mismatch/unmeasured, all four runs stable and EOF/live0/forced0. `cem-final153-candidate42-1/receipt.json`. | Representative cases, not every CEM expression or a whole-project diagnostic certificate. |
| Additional five projects, original completion/navigation cases | Release42: Moshi9, OkHttp10, Retrofit12, Spring10 and Gradle9 all match on first responses. `final-corpus153-release42/acceptance155.json`, `final-corpus153-gradle-release42/summary.json`. | These50 contain no diagnostic assertions. Spring normal's4 responses matched but its42 shutdown required Neovim termination; that operational failure is retained. |
| Spring shutdown | Release43: same original4 responses match, clean shutdown3.967s within the unchanged15s observation, no terminate call, stable inputs and EOF/live0/forced0. `spring-shutdown155-release43/acceptance.json`. | Inventory cancellation has deterministic Rust coverage, but this replay emits no source_cancelled event; it does not prove the exact formerly slow phase occurred again. |
| Declared callback parameter types | Release42: unchanged indexed10, ordinary top-level8 and member10 RPCs all match; portable semantic harness181 PASS/0 FAIL including8 new first-response completion/exact-goto checks. `callback155-42-replay/receipt.json`. | Uses declared parameter positions and generic receiver bindings. Ambiguous/unavailable declarations remain Unknown; this is not language-wide inference coverage. |
| Shared edits and scoped compiler diagnostics | Release42: Moshi16, Spring16 and Retrofit22 match, including five current-version/source-SHA compiler passes, exact Retrofit invalid range and independently applied rename edits. `shared-diagnostics155-release42/final.json`, `independent-audit.json` in the same directory. | Certificates concern the observed documents; producer_closure_complete remains false. |
| OkHttp shared edits | Release42 and46: original16/16 match with stable inputs and clean shutdown. `okhttp-shared155-release42/acceptance.json`, `okhttp-java155-release46/adjudication.json`. | Release46's new Java check remains unmeasured. The exact Java19-option producer variant is added on47; current body/three-producer/cache proof is being rerun. An empty push or supported recipe is insufficient. |
| Gradle Java6-valid shared edits | Release42:14 matched, 0 mismatched,3 unmeasured. Actual45 selective counter still has no current receipt; wire/source/prefix preserved and all cleanup succeeds. | Actual45 identifies obsolete active binary indexing blocking the next model generation. Release47 adds cooperative cancellation and archive-read aborts; selective proof is being rerun before full17. The source6-invalid default-method fixture stays separate; observation and certificate requirements are unchanged. |
| Current Java-body clean authority | Release45 and the durable48 corpus probe pass: four first-response RPCs, current errorv3 then repairedv4, exact wire/result identity, raw Javac error/success receipts and matching body-receipt digest. `java-body155-release48/acceptance.json`; portable probe published inb4ef819. | One controlled K2 module with no producer DAG. Actual OkHttp KAPT plus three normal producers still needs current native generation/body proof; no header, configuration or old binary can certify it. |
| Java-only producer, native failure route | Actual native44 invalid-only run passes with a located current Java error and clean:false. Earlier native42's valid producer/consumer phase passed, but its full run failed; native43's location failure is retained. | Current full editor canonical workflow above remains unverified. The fixed enum-name and Windows-path boundaries do not alone establish the full workflow. |
| Source-library coverage | The same Gradle42 traces resolve all six repaired POM origins without failure; aggregate failed1/missingSources29, NeKoHTML binary retention remains explicit. `gradle155-source-coverage42.json`. | Retrofit still reports six failed source origins and9 missing sources; Spring reports six and16. Binary facts do not imply recovered original-source coverage. |
| External shared-semantic extension | Current42 host and independently rebuilt child pass legacy26 plus optional46: actual model1->2/classpath3->4, stale rejection, fresh publication, crash/restart and applied rename. `project-facts143-42-final.json`; consumer4 tests/build in `external42-ready/ready.json`. | workspaceUri, source-only raw ABI and full-runtime ABI coverage remain Unknown. Full MC policy stays in mc-dev. |
| Native watcher cost | Actual Rust replay of identical real inputs reduces physical registrations Kotlin493->357 and Java487->428 while preserving logical read sets. `diagnostic155-parent-watch-runtime41/receipt.json`. | Registration counts only, not startup latency or RSS. The earlier broad-suite unrelated-invalidation failure is not deterministically reproduced and its cause remains unproved. |

The38+50 ordinary checks form a fixed checklist of representative operations.
Their completion percentage is not the goal's implementation percentage or a
grammar/whole-project coverage estimate. Plugin/build lifecycle results below retain
their actual older versions; they have not all been rerun on43. Warm latency
distributions also remain the earlier measurements.
The earlier57/88 (about65%) statement described execution of this representative
checklist at that moment, not overall goal progress. These are selected operation
cases, not an exhaustive final validation of either language.

## Previous integrated baseline39

`native-integrated120-39/main-ready.json`: workspace **1839 passed, 0 failed,
11 ignored, 42 targets**; release build passed. All600 guarded inputs remained
stable. Both owned jobs reached EOF with zero live processes and zero forced
cleanup. Raw timings remain in the receipts; these checks do not measure editing
latency or establish an isolated build-speed comparison.

Immutable binary SHA256:
`193625e15579df6f060c5018638f9fdf52b3ae3b89188de5199192c76faa2b46`.
Its path is `native-integrated120-39/bin/ktlsp.exe` under RUN (22,840,320 bytes).
The external child also builds and passes4 tests; immutable copies and hashes are
in `external37-ready/ready.json`. Each host artifact is checked against its receipt.

This binary includes the earlier local-file archive-cache and exact-declaration
fixes, archive input staging before initial watch registration, the asynchronous
semantic IPC host with exact declaration-member facts, bounded compiler-result
tracing, generic callback input/result inference, and a generic IPC publication
trace. It also content-checks source watch/save events, preserves incomplete
completion while the current owner has queued IPC work, and rejects stale Gradle
transform inputs in model caches. Actual36 pending-completion checks pass below.
Gradle's original17 improves to16 matched/0 mismatch/1 unmeasured on immutable37.
The remaining counter case is rejected before compiler execution because its dirty
Java-only dependency producer is unsupported. An actual producer compilation now
rejects that snapshot's default interface method under source6; its Java17 clean
oracle was not a valid project-level expectation. The original response and invalid
case are retained while a source6-valid concrete-class variant is being qualified.
The original9 additional corpus checks pass. Integration is not proof that all
remaining editor acceptance cases have passed.

Optional project/JVM facts are committed and pushed in
`16580dbbb7ccb3dbbb42bee3afa90ee6aeef0e3b`, including the child/probe and wire contract.
The real legacy26 and model-change46 checks pass. String operand, source metadata
and archive publication changes are committed in `0c99e4b`, `cc27816` and `9f7636c`.
The combined39 candidate contains these sources plus controlled fixture changes.
Its8 focused checks and full workspace pass. Some focused builds cleaned up an
owned MSVC helper process; those observations are retained separately from the
successful, zero-forced-cleanup integrated jobs. The candidate is not represented
as a build of the API commit alone. Six-origin POM changes pass21 focused checks
and the39 workspace. A separate strict UTF8 audit reads all3,324 source entries
(29,508,841 bytes) from the exact six selected source JARs without decoding failures;
this byte audit is not a claim that every declaration was semantically indexed.

The first35 workspace run is preserved as1820 passed/1 failed/11 ignored. Its
Windows owned-descendant test observed6.150s against an existing6s assertion.
The test now verifies its retained leaf handle signals and reports a nonzero
termination exit instead of the fixture's natural zero exit. Diagnostic and
execution assertions remain; production cleanup/timeouts are unchanged. Focused
verification and full35b pass (`slice142-validation-receipt.json`).

The next31 workspace run is retained as1807 passed/1 failed/11 ignored across42
targets (`native-integrated120-31/workspace-summary.json`). The existing
`watched_file_bursts_coalesce_and_active_refreshes_remain_singleflight` test did
not observe initial fake-Gradle startup within its12-second assertion. Its cause
was not captured precisely;31 did not build a release binary. The test now waits
for its actual invocation marker and worker terminal state, retaining overlap,
generation and invocation assertions. Focused verification and the parallel32
suite pass (`scheduler140-final.json`). No production timeout was changed.

## Earlier focused follow-up, before integration42

Ordinary numeric expressions reproduced two missing first-response completions on39
(`numeric154-result.json`). Shared numeric inference and Java primitive overload
selection now pass22 focused checks on the updated parser:17 existing JVM checks,
one Java argument-type check, two arithmetic editor-workspace checks, one widening
check and the String regression. The original failed member run is retained under
`arithmetic155-member1`; these are Rust checks, not a repaired real-Neovim result.

The valid Kotlin statement `val sum = a + b` followed by a parenthesized statement
also exposed a parser regression. The existing fork's unconditional newline-before-
parenthesis suppression is now limited to constructor contexts. Revision
`9fcb964bf2aed03aea770f54809d8a8d9a8e3311` passes all28 existing corpus cases and
two new cases; independent Kotlin2.0.21 compiler checks preserve the original
arithmetic fixture. Both workspace and external-extension locks use this revision
(`arithmetic155-parser-published.json`, `arithmetic155-pin-update.json`).

The Java-only producer route and bounded native watches are implemented but remain
unverified. The actual watcher focus currently has12 passes and two failures:
an obsolete directory-layout assertion after watch compaction, and Windows denying
a watched parent-directory rename. Both failures remain recorded; native producer,
workspace/release and the same-candidate six-project runs have not executed
(`build155-final-focus-handoff.json`). Existing integrated39 remains the latest
successful integrated build. Custom/unsigned Kotlin operators and broader overload
conversion phases are not established by the focused numeric checks.

## Current fixes and open observations

| Item | Actual evidence | Status / next action |
|---|---|---|
| Named Function0/1 trailing lambdas after named arguments | Focused semantic test passes; unchanged actual Neovim10 probes improve from2/10 on28 to10/10 on29, including5 exact source navigation targets. All first responses, stable guards and clean ownership. `named-function140-focus1/receipt.json`, `named-function140-release29-green1/receipt.json`. | Fixed for closed ordinary Function0/1, explicit/implicit parameters, Unit coercion and checked non-Unit results. Method-dependent callback types, receiver lambdas and multiple parameters remain conservative Unknown. |
| Named generic callback context and call result | A compiler-valid ordinary callback caused a stack failure before completion on33; a result-chain variant returned no useful completion. Shared context inference now avoids re-entering the body and binds method variables from ordinary named arguments. New/old focused regressions and workspace pass. Actual34 preserves both original sources:2 useful first completions and2 exact source gotos, all clean with stable guards (`completion-gap141-final.json`). | Fixed for the tested ordinary-argument-bound callback and result chain. Negative annotation/result/name controls remain conservative. This does not certify every higher-order generic or receiver-lambda form. |
| Higher-order generic callback result | Compiler-valid `transform<A,B>(value:A, block:(A)->B)` gives0/2 useful first completions on34. Context now requires known callback inputs, while the checked body result participates in the existing generic solver. Actual35b returns both `Candidate.convert` and result `Other.usefulOther` on the original first responses, with compiler proof, unchanged inputs and clean ownership (`completion-gap142-final-green.json`). | Fixed for the tested Function0/1 slice, retaining Unit coercion and negative name/annotation/result controls. Unbound results with additional upper bounds and unsupported receiver/multiple-parameter forms remain conservative Unknown. |
| Unsaved Java dependency producer used by a Kotlin consumer | Actual compiler regression counter→retracted→restored passes with fresh private Kotlin+Java producer classes replacing exactly one selected JAR slot; incomplete/foreign-owner/processor/source/ambiguous-slot/SDK/task-key guards pass. Actual original Moshi shared16 also passes on29 with a correlated current-version clean compiler certificate. `moshi-cross140-green1/receipt.json`, `moshi-focused140-shared29/semantic-handoff.json`. | Fixed for the tested single producer. Multiple producers, dirty dependency Kotlin and unsupported compilation roles remain conservative. Generic whole-project producer-closure flags stay false. |
| Unchanged local JAR CHANGED notification | Gradle29 exposes missing baselines for58 selected Project-component paths and6 tracked fixture JARs. Separate meaningful REDs reproduce both. Selected File/Project origins plus Git-cached-only archive baselines pass33 watch controls, preserving changed/new/untracked/unreadable cases. Only38 tracked JARs/124330 bytes are hashed in this actual project. `project-watch140-final.json`, `tracked-watch140-final.json`. | Actual34 original17 accepts initial GEN1 and reports zero forced JAR refreshes. Source-triggered refreshes remain a separate gap; no generic ignore of JARs under the project root. |
| Duplicate index bucket scans | Strengthened replacement test and c18 control pass. Instrumented actual method reduces retain-predicate visits24960→576, preserving the other file's64 overloads. `index-bucket139-count1/receipt.json`. | Included in29. Operation count only; no claimed end-to-end multiplier or attribution of Gradle's aggregate indexing time. |
| Same-size/same-mtime JAR replacement | Genuine warm-cache RED returns old members. New content-partitioned disk cache and atomic retirement of completed memory origins pass actual watch→model/index reload, Java/Kotlin static/instance completion and open-buffer preservation. Focused regression+controls52 executions pass, with overlapping controls. `archive-cache140-final.json`. | Included in30; actual33 same-path lifecycle47 also passes below. No sibling source archive in this reproduction. Unchanged adjacent sources can still describe outdated binary API; Project-artifact same-metadata semantic cache freshness is also unproved. |
| Exact source declaration→extension TypeId | `ReadSession::declaration_type` uses exact scoped identity and tracked lifetime/namespace/model inputs; all45 core extension tests pass. External provider uses this API and `TypeId::package()`; own-lock4 tests/build and unchanged90 editor requests/144 checks pass. `external-public140-30/{cargo,editor}-receipt.json`. | Committed/pushed in83d9e16 and7e53664. Nested owners and exact package configuration are checked in the existing unit test; no editor expectation/count changed. Members return Unknown rather than their owner/value type. |
| Exact authored declaration→extension member | `declaration_member`/wire `declarationMember` uses exact definition and a unique owner, reuses normalized member facts and tracks source/model/namespace reads. Focused core4/wire2 pass. The child uses the mandatory query instead of name matching; actual34 IPC26/linked14 and equal completion/goto/references/applied rename pass (`member141-final-receipt.json`). | Available in34. Foreign/stale tokens are rejected; ambiguous owners, incomplete facts and unresolved multiple JVM projections remain Unknown. |
| Generic IPC publication observation | `extension.ipc.accepted` is emitted only after a valid current commit reaches the shared Index. It records provider/owner/instance/evaluation/publication revision/source UTF-8 SHA256; tracing off adds no digest work. Focused host5 and actual35b IPC26/linked14 pass; fresh/restart barriers precede the first semantic RPC, and completion/goto/references/applied3-buffer rename agree (`publication142-final.json`). | Verified without framework diagnostic-marker readiness or semantic retries. This debug event is not persistent readiness or producer closure; later edits, disable and child exits retract contributions. |
| Completion while an IPC provider is pending | Actual35b returns useful authored items with `isIncomplete=false` before publication. With the same probe, child, source and request parameters, actual36 preserves every item and changes the two pending flags to true; the published response remains unchanged. All5 checks pass on first responses with clean ownership (`pending146-final-green.json`). | Fixed for queued/current-owner IPC work. Other-owner, stale, disabled, failed and accepted states are covered by host tests; the adapter also retains an empty incomplete list. The actual RED contained an item, so it is not recorded as a null-response reproduction. |
| Unchanged source watch/save events | Meaningful old-product RED observes9 refresh requests where1 is expected. Shared disk-content baselines now suppress unchanged Java/Kotlin/build-logic CHANGED/save events without using open-buffer text. Genuine same-size/mtime replacement reparses closed facts; overlays, missing/unreadable/create/delete events remain protected. Focused controls and36 workspace pass (`watch144-release36-receipt.json`). | Actual37 completes the original17 with no new source-triggered refreshes after GEN34. Initial newly selected JAR staging remains a separate race below. A source event is not treated as unchanged from size/mtime alone. |
| Newly selected JAR baseline publication | Actual37 initially knows36 readable archives, then selects67 more while notifications supersede GEN1 through34. Same-generation fingerprints are now published individually before the whole stage completes; late writers cannot overwrite an existing input or erase another stage's keys. The deterministic old-publication RED fails generation1 versus0; the new regression, related controls and integrated38 pass (`archive150-RED-proof.json`, `release38-handoff.json`). | Unfingerprinted/unknown events stay conservative. Past event-time hashes were not captured, so the33 historical notifications are not all called unchanged. A repeat end-to-end reduction from34 generations has not been measured. |
| Repeated dependency-source query after source refresh | Real S1/B2 captures have identical194-module binary DTOs, unchanged build inputs and complete sources/POMs for all1176 source sets. The old model references a deleted Gradle transform snapshot. The shared5 validation paths now track205 deduplicated declared transform files for presence/mtime. Actual36 naturally misses the stale cache and saves complete source/POM selections for all194 modules/1176 source sets,49,390 source refs/72,307 POM refs; the owned warmup closes cleanly (`gradle-shared134-native36/final-sidecar141.json`). | DTO comparison stays strict. This proves selection acceptance/cache persistence, not whole-root indexing:10 origins fail and38 sources are missing.205 is metadata operations per cache candidate, not a measured latency claim. |
| String-concatenation receiver completion | Actual36 returns null for4 compiler-valid concatenated receiver requests. Actual38 passes the unchanged original8 plus4 typed/exact-location checks; the Object/Any?/null additions return all4 required completions, once each (`string153-38-adjudication.json`). Separate first responses offer the String-only extension for both Kotlin concatenations and exclude it for a useful CharSequence control; pinned compiler positive/negative eligibility agrees (`string-owner153-independent-adjudication.json`). | String eligibility is established independently of display text: kotlin.text substring also appears on CharSequence. Both overbroad observer exit1 receipts are retained. An unrelated whole-candidate-pool Unknown is not reclassified as Found or called full-harness success. Numeric/custom-left operators and general language parity remain outside this tested intrinsic slice. |
| POM metadata encoding | Strict POM-only CP1252 decoding plus six-origin conditional/compiler metadata interpretation pass21 focused checks and integrated39. The exact12 selected POMs prove all6 UTF8 policies; all3,324 actual source entries decode strictly. Real library38 passes43 checks, including exact Selenium WebDriver.get source SHA/range (`pom153-selected-source-bytes.json`, `library153-editor-1/verification.json`). | Java-source encoding policy is unchanged; mixed/unknown candidate encodings remain rejected. A full-root post-fix index count is not yet measured. NeKoHTML has byte E9 in a comment with no encoding metadata; binary facts/stub navigation remain available, but original source coverage is not invented. |
| Library probe readiness | The old151 scenario observes all250 listOf retries before source installation; Java negatives precede JDK completion and Selenium uses a binary-only stub. The existing probe now waits for the same accepted/dependency generation and successful indexing progress end, then uses first responses. Actual38 passes all original42 checks plus one coverage check, with EOF/live0/forced0 and all buffers restored. | Old151 artifacts/logs were restored byte-for-byte at their original paths. New artifacts and the explicit relocation map are under `library153-editor-1`; this is one semantic verification, not a new performance distribution. NeKoHTML still uses binary facts/generated stub navigation, not recovered original source coverage. |

## Historical project and lifecycle acceptance

These are finite selected workflows, not whole-project absence-of-errors certificates.
No project source is rewritten for tests: unsaved edits or disposable private copies
are used, with original contents and Git metadata guarded.

| Requirement / project | Latest retained result | Remaining verification |
|---|---|---|
| CEM excluding Mixin | Actual32 closes36 matched/0 mismatch/2 unmeasured out of38: normal7/9, incomplete17/17, getter8/8 and TypeTools4/4. All owned runs reach EOF with stable guards, no live children or forced cleanup. Both unmeasured checks require canonical Java diagnostics; raw javac output contains the intended error, but canonical publication/return was unproved on32. `cem-remaining140-release32.json` links the separate receipts, including the failed normal stage. | The two canonical checks pass in the33 follow-up below. Preserve32's partial result; do not call it a same-run38/38 or a whole-project certificate. |
| CEM canonical diagnostic follow-up | Release33 now passes both original canonical checks with current-v3 certification and both buffer-preservation controls. Exact source SHA is audited;447 guards stable, EOF,0 live/forced cleanup (`cem-startup120/editor-original26-diagnostics140-release33-warm1/acceptance-release33.json`). Earlier debug follow-up also passes and remains retained. | The two previously unavailable32 checks are verified on33. No diagnostic semantic fix was required; timing evidence supports prior work exceeding the observation window, while its exact old finish gate was unobserved. This is not a whole-project or latency certificate. |
| Additional five projects | Actual32 verifies41/41 first responses: Moshi9, OkHttp10, Retrofit12 and Spring10. Actual34 now passes Gradle normal3/incomplete6, completing the original50 across these releases. All stages have stable guards, clean LSP shutdown, EOF and zero live/forced cleanup (`final-corpus140-release32/independent41-summary.json`, `final-corpus141-watch34/{g-normal-native1,g-incomplete}/receipt.json`). The exact missing SDK34 prerequisite was restored from verified official archive bytes before Retrofit ran. | Selected50 achieved. These contain no diagnostic expectations and do not certify all syntax or full-project IDEA parity. Gradle's separate shared-edit diagnostic gaps remain below. |
| Moshi mixed shared edits | Original atomic6 passes on28. Shared16 improves from15 matched/1 unmeasured on28 to16/16 first responses on29, including current Kotlin1.8.21 counter diagnostics. Original/copied sources and JAR unchanged; original missing producer JAR stays absent. `moshi-focused140-shared29/semantic-handoff.json`. | Tested workflow achieved. Editor API retraction completion/goto passes; compiler retraction diagnostics are separately established by the native lifecycle case. |
| Spring mixed shared edits | Original16/16 first responses pass on28, including current-version complete compiler pass certifying empty document diagnostics.10861 original tracked inputs unchanged. `spring-shared138-final.json`. | Whole-project producer closure remains false; do not broaden the document-level claim. |
| Retrofit mixed shared edits | Portable original22 first responses pass on27; exact invalid-diagnostic range separately audited against the same payload. Applied rename inspected. `retrofit-shared137-portable27/final-summary.json`. | Final source changes must retain the existing expectations. |
| Gradle mixed shared edits | Actual37 closes16 matched/0 mismatch/1 unmeasured. The invalid and fixed canonical checks pass; the counter remains unverified. Native nvim exit1 accompanies normal LSP shutdown/exit0, no forced cleanup or surviving children,19 guards stable, and all25,569 tracked files unchanged (`gradle-shared134-native37/receipt.json`, `gradle-shared134-tracked37-final.json`). | Counter sourcegen4 is rejected as dirty-visible-dependency-source-unhandled before compiler launch. A real Java11/source6/ErrorProne/processor compilation rejects its default interface method; the old Java17 clean oracle was incorrect for this project. Preserve that result, qualify a concrete-class Java6-valid variant, and implement the validated producer route (`diagnostic153-cause.json`, `diagnostic153-producer3`). Do not increase the300s window or label fast empty diagnostics canonical. |
| Libraries added/replaced by real Gradle builds | Actual33 versioned90/61 first RPCs/4 builds and same-path47/23 first RPCs/2 builds both pass. New API, old API withdrawal, exact source navigation, current diagnostics and unsaved-buffer preservation are checked; all8 guards stable, clean shutdown/EOF/0 live/forced cleanup (`dependency-lifecycle141-final.json`). | Tested lifecycle workflows achieved. Missing dependency and source archives were restored by genuine Gradle resolution; the original offline acceptance stayed unchanged. Failed setup/readiness runs are retained separately, with no latency claim. |
| Compiler plugins / generated sources | Historical32 has109/114 pass and correct0,0,1(disabled),0(restored) builds. The5 unavailable/mismatched phase1 requirements now pass within original focused8 on33: Java/Kotlin each one diagnostic response, exact3 obsolete calls per language, current v4 SHA/result identity,2 actual builds/RuntimeCheck12 each,32 stable guards and clean EOF/0 live/forced cleanup (`processor120/processor-phase1-release33-resume141b/semantic-adjudication141.json`). | Focused semantic gap closed. The original cold indexing120s check still fails, so that run is34/35 and exit1; do not call the full114 rerun or all35 successful. No semantic retry or deadline increase; producer closure remains false. |
| External shared-semantic extension | Embedded exact-declaration provider: own-lock4 tests/build and Neovim90 first responses/144 assertions pass (`external-public140-30/editor-receipt.json`). New actual child IPC26 checks/12 extra RPCs and linked14 checks/6 RPCs both close cleanly with stable guards, EOF and no live/forced cleanup. Same-input completion edits, goto, references and applied3-buffer rename match (`ipc140-editor-2/receipt.json`, `ipc140-editor-linked-1/receipt.json`, `ipc140-linked-equivalence.json`). | Single Java shadow-method IPC spike achieved, including cancellation/stale rejection, idle-crash index retraction, restart and disable. Kotlin/fields/unopened-dependency wire acceptance and full MC policy remain separate. The first fixture-restoration failure is retained in `ipc140-editor-1`. |
| Real mc-dev companion integration | Stock35b/36 plus the independent MC child each pass112 checks/29 single-attempt requests. The published optional API adds real37 legacy26 plus model-change46 matched checks: actual classpath3→4, old evaluation Stale, fresh completion/goto/references/diagnostics and applied rename (`project-facts143-37-final.json`). Candidate37 serializer copies match both guarded source snapshots (`candidate37-semantic-handoff/manifest.json`). | Source Shadow/method and two-operation integration achieved; full MC commands/resources/bytecode are separate. workspaceUri, source-only ABI/classFlags and full-runtime ABI coverage remain Unknown. Candidate binaries include other working-tree changes and are not claimed to be builds of16580db alone. |
| IDEA comparison | Selected original50 first responses are independently qualified against IDEA: Moshi9, Retrofit12, OkHttp10, Spring10 and Gradle9. Authority/receipt hashes10/10 still match. CEM getter method/property semantics have independent compiler proof; IDEA's separate property insertion also matches the exact buffer. The original method-label mismatch remains retained. | General IDEA parity remains unmeasured. External oracle shutdown cleanup is an operational limit, not a new semantic acceptance gate. Historical CEM bounds receipts in Temp are no longer present and were not reverified. |
| Warm responsiveness | Earlier release warm30 RPC p95 18.577ms; input-to-menu p95 52.326ms; warm startup n5 p95 1062.820ms. | Latest distribution unmeasured; user currently prioritizes Unknown coverage. These are not pixel-display timings. |

## Extension boundary and mc-dev handoff

The contract supports compile-time Rust composition through
`Backend::with_analysis_providers` and explicit stdio child registration through
`analysisProviders.<id>.command`, using shared semantic contributions and tracked
invalidation. The versioned JSON-RPC bridge resumes validated owned pending state,
preserving snapshot identities, tracked reads, cancellation and stale rejection.
Communication runs outside Workspace locks. There is no stable dynamic ABI or
certification of full production MC policy.

The human-approved `mcdev-extension-boundary-reply.md` was sent to the mc-dev task.
It recommends stable commit `646d8277e22e53de1a9842c2a74b700f88dc28fb` for the
existing API, whose public extension source matches the independently built4-test
checkpoint. This original pin does not contain the new declaration-type method;
the method and external consumer are now committed through7e53664.
The separately human-approved `mcdev-ipc-policy140.md` was also sent for policy
review only (SHA256 `4b52a4900dc714e1144d8cb5d27ca68dedb9b378ec98bf920535d23f30f92c92`).
It recommends a long-lived stdio JSON-RPC child with asynchronous snapshots,
host-issued identities/read tracking and atomic checked contributions, starting
with single-target Mixin/Shadow. That message only transmitted a policy proposal.
Subsequently the mc-dev migration task received a new user instruction to replace
its implementation with Rust, visible in its current task preview. Its requested
general host IPC spike is now a separate next slice: publish the protocol boundary
and prove delayed-child/unsaved-edit stale rejection without blocking ordinary
completion. The direct human instruction to continue implementation overrides a
subsequent mistaken pause request from the migration task. Core pending/resume
and framing checks pass (2+1 tests in `ipc140-core-1` and `ipc140-lsp-1`). The
later host controls pass4 tests and diagnostic lifecycle controls pass7 with one
existing native ignore. Actual stock-host/child editor barrier, same-input linked
comparison and cleanup now pass as recorded above. This establishes the bounded
single-method spike, not full extension or MC parity.
Provider-specific structured diagnostic codes are a recorded nonblocking follow-up;
do not introduce a broad diagnostic enum/transport refactor for the current migration.
Full MixinExtras/Lapis, generated/multiple-target and complex generic/value-class
semantics are not certified by the test extension.

## Next actions and execution rules

1. Complete the Java6-valid dirty-source counter using Kotlin's ordinary Java
   source/header inputs and the original classpath. Keep Java-body authority
   separate; do not restore automatic Gradle archive replay or KAPT/KSP execution.
2. Retain the actual external Lombok75 and KSP/KAPT114 proofs. Preserve
   configuration changes, stale API withdrawal and unsaved-buffer behavior in
   affected later verification.
3. Retain the six-origin POM/byte proof and binary fallback limits; investigate real
   failed source origins without guessing an encoding. Broader operator, bounded
   generic and receiver-lambda inference remains outside proven coverage.
4. Use one coherent final snapshot for affected editor/plugin/build lifecycle and
   latency checks. Reuse existing controllers, goldens and genuine caches, repeating
   only checks affected by later changes. Cold setup remains a separate measurement.
5. Update this record, selectively commit/push verified changes, and report feature
   evidence and gaps. Do not derive implementation or grammar percentages from counts.
6. Keep the verified asynchronous IPC boundary usable by the independent mc-dev
   consumer; later fact capabilities remain scoped additions, with MC policy in mc-dev
   and no communication waits under Workspace locks.
7. Publish the verified mc-dev source inventory/current snapshot slice with its
   serializer fixtures and real child evidence. Keep large-scope handler/budget
   proof distinct from complete large-project editor proof; resources, raw class
   bytes and command routing remain separate contracts.

The human permits ongoing API-contract, implementation-commit and verification-result
sharing with the same mc-dev task within this goal. That scoped technical coordination
does not require another permission request for each message.

Native delegated work uses gpt-6.1-sol/high, at most5 agents, no new worktrees.
One Cargo owner and a global Rust write freeze cover each native build. Immutable
editor runs can overlap later source work. Cache directories are private; stdout
stays JSON-RPC only. Keep raw logs on disk and read bounded summaries or exact errors.
Preserve user AGENTS.md, unrelated untracked files and historical failure receipts.
