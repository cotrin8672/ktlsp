# Current fork state

Read this before the historical acceptance record. Goal remains in progress.
Updated2026-10-16 (Asia/Tokyo), checkpoint86 base verified; goal incomplete.

Previous checkpoint86 result: fmt, the serialized-symbol wire regression,
the full workspace suite (1,542 passed, zero failed, four ignored), and release
build pass. All four owned stages close with EOF, zero residual cleanup and
stable input guards. The root reviewer independently checked receipt stages,
aggregated raw test results and the immutable binary (19,216,896 bytes, SHA-256
cf9ebc7655eb2450b9168181e5954cd3edc651d14ead4bcda1a7afa154a3bb78).
Evidence: native-binary-shadow-release86-3/{receipt,binary-reference}.json.
This supersedes the pending combined Rust checks below. Latest CEM passes the
unchanged original10 requests plus all8 separate Java assertions, split between
ordinary and incomplete-code stages. Both stages exit zero with EOF, stable
source guards, no forced cleanup and no live owned process. The root checked
both result counts and artifact hashes against this exact immutable binary.
Evidence: native-cem-shadow-editor86-1/receipt.json. Broad Java editor acceptance
remains pending; CEM18 does not establish whole-project IDEA parity or latency.
The CEM timing audit records project scan completion at89.738/104.510ms,
dependency indexing at51.415/42.111s, and the first useful original completion
request starting at51.437/42.123s, with server spans4.024-6.361ms. These runs
use fresh analysis caches, a copied disk model and warm Gradle downloads. The
harness waits for dependency readiness before sending original requests, so
these timings do not prove the earliest usable completion or display latency.
The dependency spans include source queries34.791/26.038s. Measure requests
while a source query is pending before and after the publication-order fix.
Evidence: native-cem-shadow-editor86-1/timing-summary.json.

The separate Rust extension fixture now passes its actual Mixin-artifact javac
control, crate test/build and real-Neovim corpus proof: two original and fourteen
additional RPCs match, including applied prefix-preserving rename, unsaved
target changes, configuration and disable/re-enable invalidation. All six native
stages exit zero with EOF, no forced cleanup and stable input guards. The root
reviewer inspected saved renamed buffers: both shadow$doubled uses retain their
prefix and the unrelated target stays unchanged. Evidence:
external-shadow86-1/{receipt,editor/artifacts/external-shadow-proof}.json;
portable reproduction: dev/external-extension/README.md.
This proves primitive-member executable composition. Nominal reference types
such as BlockPos, production Mixin support and IDEA parity remain unproved.

The production Gradle init script passes PREFER_SETTINGS and PREFER_PROJECT
with the same GAV under different repositories and an isolated shared Gradle
home, preserving exact selected source paths for main/test source sets. This
does not prove editor refresh or compiler-plugin behavior. Evidence:
native-gradle-product-source-policy86-1/receipt.json. Latest annotation/import
editor verification closes with six matching first responses (four useful
Java/Kotlin plain/qualified annotation completions and two unknown-package
negative controls), and two applied Java imports whose exact returned source
texts compile with javac. Alias compiler and selected-SDK archive-byte/range
controls also pass. The root independently checked these artifacts. The broad
Java scenario still fails five diagnostic expectations after library readiness:
unresolved member, call shape, argument type, and two changed-superclass cases.
Its editor stage exits1; the passing annotation subset does not make the scenario
pass. All stages close with EOF and no residual termination. Evidence:
native-annotation-editor86-3/receipt.json. Actual
dependency/generated-source AUTO-WATCH refresh closes at84/86 checks: the updated
Java caller's stale member has no expected diagnostic, and same-client shutdown
times out. The owned job records one residual termination and no live process;
this is a failed run, not clean shutdown or full refresh acceptance. The model
does contain queried, complete exact source selections for all four source sets.
Evidence: native-gradle-reload-editor86-1/receipt.json. Next: fix current Java
diagnostic invalidation and inspect the shutdown failure; preserve both failed
expectations. KAPT/KSP refresh has not run on this release. The next implementation
must also publish useful selected binary facts before slow online source queries,
retain a resolved source when an unrelated component fails, and certify the POM
origin used for source encoding. These are open correctness/performance gaps,
not acceptance of the current Gradle integration as complete.

Checkpoint87 is in progress after publishing checkpoint86 as775cf4e. An actual
token-level regression reproduces selected UTF-8 sources being decoded using a
foreign same-GAV ISO-8859-1 POM: the expected `café` member disappears
and a corrupted identifier is indexed. The test compiles and fails semantically
(one failed test, exit101, EOF/owned0/stable guards). Evidence:
native-pom-origin-red87-1/receipt.json. Runtime-faithful Java tests also reproduce
missing local and inherited argument errors without the test-only completeness
override; the independent javac/Kotlin controls confirm the expected errors.
Evidence: native-java-diagnostic-compiler86-1/receipt.json.
Implementations are assigned for exact POM provenance, early binary publication
and source-query cancellation, and bounded current-source hierarchy certainty.
These changes have not yet passed combined checks.

Focused87-2 passes23 tests and fails two, with fmt separate: the certified POM
selection/encoding filters pass17 tests; Kotlin inherited-call diagnostics and
an unchanged in-root POM watch expectation fail. The watch predicate was fixed
without weakening its unchanged/changed/deleted expectations. Kotlin's global
producer gate now delegates member calls to the bounded owner/hierarchy proof;
unqualified top-level calls retain the global gate. Actual no-model JDK batches
also retain their provenance, rather than relying on injected test origins.
Focused87-4 then passes seven tests: runtime Java/Kotlin diagnostics and uncertain
owner controls, exact source/POM watches, useful Java/Kotlin completion while a
source query is pending, continuity after model enrichment, and owned native
child cancellation. The root checked each raw result and stage. Those stages
close with EOF, no forced cleanup and no live owned process. A separate actual
binary override reduction in the same recipe still fails two inferred-result
expectations; completion and exact definition already pass. Its final native
stage records one residual termination, so the overall recipe is not accepted
as wholly green or clean. Evidence: native-combined-diagnostic-focus87-{2,4}.
The reduction needs a control without the unsupported ancestor and a return-chain
completion check before attributing the failure or changing binary metadata.

The actual Gradle reload failure has a selected-SDK mismatch: phase2's compiler
recipe uses JDK17, while the indexed SDK source origin is JDK21. Shared selected
SDK indexing/retirement is in progress; substituting an Object-only adapter would
leave navigation/completion inconsistent. Preserve CEM's selected SDK21 and
same-feature release settings; multiple SDKs need source-set scope, not a first
winner or a host-JDK fallback. Latest broad Java and Gradle editor reruns remain
required after the combined release.

Actual parent-POM discovery passes both repository policies in an isolated SDK21
Gradle home. Six owned stages (javac/javap controls plus production source tasks)
exit zero with EOF, no forced cleanup and stable guards. The project policy
exports the actual ISO-8859-1 encoding parent even though it is absent from the
runtime dependency graph; the settings policy selects the competing UTF-8 root.
The root checked exact SOURCEARTIFACT/POMARTIFACT rows as well as the receipt.
An earlier attempt stopped at a javap stdout-encoding assertion before Gradle;
it is retained as a control/tool failure. Evidence:
native-gradle-parent-policy87-2/receipt.json; pinned production init script
67064938663fcfe4a231ec8ebbdc72374ca68dc73c2512a8996d298a04174b59.
This establishes public Gradle selection and compiler oracles. Host consumption
through a fresh Neovim process and the portable fixture remain pending.

Actual fresh MODEL14 captures now succeed for Retrofit, Moshi and OkHttp, with
their original source bytes retained. Moshi's regenerated required JAR is present
and its Java/Kotlin task snapshots are complete. OkHttp's three absent Java output
directories belong to Kotlin-only tasks with zero Java sources; they are not
missing required compiled artifacts. Spring is still operationally unmeasured
because its private Gradle8.10.2 immutable cache lacks metadata. Batch repair of
only proven invalid generated workspaces is authorized; accepted models remain
valid. Latest five-project editor acceptance is still unrun. Evidence:
native-five-{retrofit-current-model86-2,moshi-current-model86-1,okhttp-current-model86-1,spring-current-model86-1}/receipt.json.

The processor-refresh attempt stops before LSP startup: the offline Gradle cache
lacks the pinned Kotlin JVM1.8.21 plugin marker. This is operationally unmeasured,
not evidence for or against KAPT/KSP editor semantics. The owned stage closes
exit1/EOF/owned0 with no forced cleanup. Bootstrap the exact pinned dependencies
online, then rerun the unchanged offline probe. Evidence:
native-gradle-processor-editor86-1/receipt.json.

The remaining checkpoint86 notes below retain earlier observations and gaps;
the verified results above supersede their pending-check descriptions.

Keep tool output bounded: read this record and changed fields first, retain full
logs on disk, and inspect failure excerpts only. Reuse successful checks until
relevant inputs change; agent handoffs contain only delta, result, evidence and
next command. Do not re-read full guard inventories or historical log files.

The public Gradle source query passes PREFER_SETTINGS, PREFER_PROJECT and the
actual CEM Create component (three native stages, exit0/EOF/owned0). The Create
source archive is13,014,998 bytes and its expected source entry matches. This
proves repository resolution. An all-module probe selects107 components and
obtains100 source artifacts: query51.987s, process84.172s, one partly-cached
sample (source jars21 to118), not a cold/warm latency distribution. Seven
components have no source artifact; this is distinct from query errors. An
eager query would delay initial project publication. The product integration
must also preserve exact Gradle-selected source paths: cache lookup by GAV can
pick another repository's source, and file repositories need not populate the
Gradle modules cache. Deferred resolution and warm controls are under review;
The exact-source model/helper changes are being implemented; worker hookup and
editor validation are pending. Evidence:
native-gradle-source-query85-2/receipt.json;
native-gradle-source-query-all86-1/{receipt,cost-audit}.json.
Commons1.26.0 real-Neovim completion returns putArchiveEntry on the first request.
The selected POM policy is ISO-8859-1 and source parsing succeeds (zero failures),
so the archive's non-UTF8 bytes do not establish a source-error fallback case.
That editor proof remains open; do not count the intended-error assertion as
product failure or fallback acceptance. The valid build/model are reused.
Evidence: root-commons-source85-audit.json; native-commons-binary-fallback85-4.
Spring/Gradle temporary corpus checkouts are clean after enabling local Git
longpaths; source files and revisions are unchanged. Evidence:
five-project-next84-1/root-longpaths85.json.
Latest CEM editor86 passes the unchanged original10 requests and all8 separate
Java assertions. Each original request receives a matching first response;
both stages exit0/EOF/owned0 with clean shutdown, zero forced terminations and
stable source guards. The root reviewer checked artifact hashes and result
counts. The missing hierarchy fact was Ponder's actual VirtualBlockEntity
interface; preserving its two abstract methods plus Create's nominal hierarchy
restores super-member and generic-extension completion/navigation. Focused
binary10, fields-only1 and implicit-receiver66 checks pass; full Rust1,528 pass/
0 fail/4 ignored, fmt and release pass. Workspace/release cleanup each records
one owned residual termination; these are retained separately from editor
shutdown evidence. Evidence: native-cem-ponder-{focused,release,editor}86-1.
Independent review found another safety gap: omitted abstract or unsupported
overrides can expose a concrete ancestor after binary hierarchy publication.
That case is reproduced and fixed with normalized per-owner/per-kind unavailable
member facts, exact abstract declarations and shared completion/navigation
filtering. The actual javac-produced hierarchy regression passes both languages,
local/module and cold/warm routes. Focused86-4 passes binary12, fields-only1,
annotation3, source-model3 and HTTP/cache2; the core members filter matched zero
tests and is not counted as validation. Both the root and independent reviewer
checked closed receipts. Global symbol cache is29 and binary cache schema v5;
Gradle model remains14. Full and editor checks of the combined slice are pending.
Latest five-project50, repository-source integration,
live dependency/plugin refresh and updated performance comparisons remain open.
Fresh Moshi MODEL14 exposes a missing main classpath JAR and an incomplete
Kotlin task snapshot. The exact :moshi:jar task regenerates it successfully;
model recapture and editor acceptance remain pending. Retrofit capture fails
on an empty private Gradle Groovy-DSL cache leaf; only that generated leaf is
quarantined with a receipt, preserving the rest of the cache. Evidence:
native-five-moshi-{model,jar}86-1; retrofit-groovy-cache-repair86-1/receipt.json.
An annotation editor baseline reproduces empty completion for Java
@java.lang.Over despite useful @Over completion. Unknown-package controls stay
empty. Kotlin @kotlin.Depre lacks a stdlib input in that fixture and is not RED
evidence; a JDK-backed Kotlin annotation control is being established. The broad
Java harness also exposes six diagnostic/navigation failures; its run is not
accepted. Audit identifies an obsolete SDK source hash/range assertion (the
returned range matches the actual SDK) and three diagnostic probes sent before
library readiness, which cannot prove ready-world diagnostic behavior. Correct
SDK-derived expectations and post-readiness edits are pending. Two superclass
diagnostic cases fail after readiness and remain unattributed. Evidence:
native-annotation-editor86-1 and its captured SDK/resource artifacts.
Independent annotation compiler controls pass15/15, including Java/Kotlin
private-nesting distinctions; the corrected editor run is pending. Evidence:
native-annotation-compiler86-1/receipt.json.
Extension API supports a separate Rust executable through public Backend
provider registration. An external fixture is being verified; stock executable
runtime loading is absent. JvmType currently lacks nominal reference types,
so primitive-only extension tests cannot prove Minecraft member signatures.
Reference-type support and external enabled/disabled/edit/config/rename proof
remain required. Production Mixin support stays outside this project.

Checkpoint85 implementation: typed Gradle input roles and source-error binary fallback. Typed KGP snapshot roles remove
only Gradle IC fingerprints from fresh-CLI reads; registered and explicit CLI/
plugin argument reads stay watched. Source/CP/plugin/SDK overlap fails closed.
The native watch cap and nonrecursive coverage are unchanged. Two strict RED
cases reproduced the old behavior; ten new focused tests, fmt, Rust1,524 pass/
0 fail/4 ignored and release pass. Working cache schema is14.
SDK17 actual MODEL14/editor control passes: source7 unchanged, registered3
snapshot inputs captured and retained, original12 requests plus auxiliary4 match.
One current Java pull proves SDK17/null-encoding, current wire/version and exact
ERROR, with canonical Kotlin collector byte equality and source restoration.
Native3 stages exit0/EOF/owned0/unforced. A final checker schema bug was fixed
file-only; no native rerun. Evidence: root-sdk17-acceptance84-1/audit.json.
Source-error fallback85 keeps verified selected-module binary members when a
source archive fails parsing. Failure/incomplete coverage remains reported and
no origin is marked complete. Strict RED:1 semantic failure, then focused GREEN:
1 test passes with corrupt-ZIP/invalidUTF8, Java/Kotlin and cold/warm controls.
Two initial test compilation errors were repaired and are not RED evidence.
Full Rust1,524 pass/0 fail/4 ignored, fmt and release pass. Editor proof of this
source-error fallback is pending. Evidence: native-k2-java-{red85-3,focused85-1,
release85-1}; module-source-error-fallback85-root1/root-review.json.

CEM actual regeneration/MODEL14 capture now pass. Original118 source hashes
remain unchanged; all565 old guard rows have current inputs, with no missing
paths and no removed guards. The one regenerated NeoForge artifact has two
explicit old/new guard rows;563 rows retain their exact old bytes. Actual typed
roles contain101 Gradle IC snapshots and zero registered-read overlap.
Native2 stages exit0/EOF/owned0/unforced. Evidence:
root-cem-recovery84-1/audit.json; kl84-cem-recovery1/baseline-proof.json.
CEM editor84 observed: original10 give1 match (negative control)/9 mismatches;
all8 separate current-Java assertions match (one pull perstage), canonical
collector bytes/current wire+version/exact import ERROR/restoration pass.
The native watch cap is resolved. Both stages are closed exit1 semantic failures,
EOF/owned0/unforced; launcher exit0 records completed observation, not acceptance.
Library indexing records one failure: commons-compress1.26.0 source text has an
invalid UTF8 byte. Query traces report incomplete scope/unknown receiver; binary
fallback and completeness propagation are under investigation. Keep all original
expectations. The CEM floor is not accepted.
Create negatives were inherited from the fresh capture cache into both editor
runs. Older83 had actual Create sources;84 fell back to public-only binary stubs,
with neither protected getBreakSpeed nor extends. These are different source
availability conditions, not proof of a watch-planning semantic regression.
Source lookup currently lacks the declared Create Maven repository.
Next: verify source-error fallback in Neovim; probe Gradle source resolution
under repository policies, implement binary hierarchy/protected access, then
original10 and latest five-project/live-Gradle verification. Evidence: native-cem-java-floor84-1/receipt.json.

- Implementation83: exact current SDK21/explicitUTF8 Java recipe with selected
  Kotlin2.4.0/Gradle9.6.1 BTA DAEMON facts; SDK17/K1 retained. The collector remains
  a standalone CLI: original clean compilation and every output byte must match.
  Configured encoding and actual JVM default charset are recorded separately.
  MODEL13 refreshes previous captures once. Five regressions failed before the
  implementation; focused checks, fmt, Rust1,514 pass/0 fail/4 ignored and release pass.
- Private SDK21 native proof: independent actual Gradle API options match all19
  captured options; five Java class files match every byte/major65, valid compile
  succeeds and unused import produces exactly one missing-package ERROR.
  Actual Gradle DAEMON marker/no fallback; all four Kotlin collector outputs
  match canonical bytes with current eight-source identities/package origins.
- Current CEM: actual MODEL13, complete source/classpath/task capture, SDK21/
  explicitUTF8/DAEMON, Java19 API options. Original source118/91 nonMixin unchanged.
  The new editor run matches four original golden requests (two completions,
  two definitions) but does not
  obtain current Java compiler diagnostics: native input watching hits its cap.
  Additional probe: two matched restoration/input assertions, two unmeasured,
  zero manual pulls. Incomplete6 was not reached; original floor10/UI31 remain at82.
  All25 Java sources are Mixin/Accessor files; the91 nonMixin sources are Kotlin.
  A Java unused-import probe there proves only ordinary import diagnostics,
  not Mixin semantics or nonMixin Java coverage.
- Real Neovim UI31: one unique CEM context, first1/warm30; all31 accepted exact
  text with one completion request each. Warm menu handoff p50=42.531ms,
  p95=55.615ms (nearest rank); RPC p50=9.878ms/p95=21.797ms.
  No physical-screen or IDEA/performance-budget claim. Deferred acceptance-text
  verification time is excluded from insertion latency.
- CEM startup, one sample per condition: fresh ktlsp cache with warm Gradle/deps
  project27.969s/library43.532s; closed UI cache reuse project93ms/library1500ms.
  Warm run has one disk model hit and zero Gradle dumps. Server RSS only:
  cold steady/peak1849.4/1854.6MiB; warm1373.7/1373.7MiB,1s/11 samples each.
- Reproduced current blocker: compiler-native-watch-cap before compiler acceptance,
  not an empty successful diagnostic result. Bounded audit:225 selected inputs
  require628 distinct nonrecursive directories against cap512; declared inputs
  include101 Gradle classpath snapshot artifacts. Do not just raise the cap or
  drop inputs by filename: preserve exact source/config/dependency invalidation.
- Next: fix input watch planning, rerun original10 plus Java probe and SDK17 editor
  control, then latest five projects/live Gradle refresh. No current CEM Java
  diagnostic parity claim. Keep existing timings historical until remeasured.
- Last complete six-project floor remains release76/checkpoint77 (60 RPCs).
  Only CEM is current at82. Other5/IDEA comparison/broader plugins/inherited and
  generated Java coverage/wider UX remain open. Goal is not complete.
- Five-project preflight found missing authored/config files in private Temp
  fixtures. All10 original manifests (50RPC) and exact pinned Git objects survive.
  Restoration receipt passes:416 authored sources,360 configs and1,661 other
  tracked inputs restored, all2,437 frozen hashes matched (265 exactCRLF).
  Existing35,397 files plus one symlink preserved; no Git metadata/generated
  outputs/model/cache restoration. Active-oracle/Git readiness and actual Gradle
  regeneration/MODEL14 capture remain pending. Evidence:
  five-project-next84-1/{audit.txt,restoration-receipt.json}.

## Efficient continuation

Read this file first. Open historical evidence only for a specific unanswered question.
Keep raw logs on disk. Before printing JSON, select scalar fields and check its shape;
never print whole models, ABI tables, guard inventories or stage/request arrays.
Normal output: status, counts, artifact path and the next action. Bound output at
the producer as well as the tool; on failure retrieve only the relevant code/log range.
Default read budget: at most80 lines/4,000 characters per call. Search for the
symbol first, then read its relevant range. If more is needed, narrow the question
and request another range; tool truncation is not an output filter.
Select primitive JSON fields before printing; check dict/list types before listing keys.
Never print progress arrays; inspect a closed receipt and aggregate its stages instead.
Do not dump a plan to inspect its command.
Aggregate test counts across suites and treat zero matching tests as unverified.
Count original requests from expected_requests and request_outcomes, separately
from combined matched totals that include auxiliary probe assertions.

Run focused checks during iteration and broad checks after a settled semantic change.
Reuse successful evidence until changed inputs or an unresolved concern justify a rerun.
Reuse existing harness/helpers; separate unique cases from repeated observations.
Agents use sol6.1/high and return only changed findings, paths and next action in
at most10 lines. Do not repeat completed investigations in delegation prompts.
Persist runnable probes and a short launch state so compaction needs no log replay.
New agents receive a narrow task and evidence paths with fork_turns=none;
reuse existing agents without repeating their completed investigation.
Restart completed agents with followup_task; send_message alone does not restart them.

Stop dependent tool calls when preparation fails. Rebind intentionally changed
fixture/common-input guards explicitly to the tested release before editor launch.

Evidence base: C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1.
Current functional gaps confirmed on2026-10-16: accepted Moshi Java compiles
with SDK17 and release8, which the selected SDK view currently refuses; this is
an API-view coverage gap, not SDK completion acceptance. Retrofit actually uses
SDK8, whose selected source archive is home/src.zip rather than home/lib/src.zip;
the current selector needs that same-home layout support. Gradle launcher SDK17
does not establish the compilation SDK of every module. Five retained the four
actual source-set combinations in accepted-four-sdk-targets86.json.
The actual IDEA server263.6379.0 ran both portable source-policy imports through
Neovim: seven of eight semantic expectations matched; inherited ISO source cafe
navigation used the selected class instead of the expected source location.
Both imports completed, but each comparator shutdown exceeded the harness
observation window and required one child cleanup. Tooling/daemon SDK22 was
observed despite requested21; project SDK is unproved. This is a limited oracle,
not stock-ktlsp comparison or general IDEA parity. Evidence:
native-gradle-parent-idea87-3/receipt.json. Stock comparison awaits new release.

Checkpoint87 release is now verified: native-editing-sdk-release87-4 has fmt,
full no-fail-fast workspace1,563 passed/zero failed/four ignored, and release
success. Root audited raw counts/hashes, stable guards and all three owned stages
with EOF, no residual cleanup and no live child. Immutable binary:19,299,840 bytes,
SHA-256 d1d133307da739327d6b6cbf08bfd3b16e64ea2fb176dd58427c81b897b31d61.
Real Neovim Java editing passes345/345, plus six exact annotation first responses
and two actually applied import edits whose full texts compile. Evidence:
native-java-editing-editor87-1/receipt.json. Its Lua uses forced client stop, so
this proves semantics, not graceful LSP shutdown; that harness correction and
clean shutdown verification remain pending. New CEM/reload/processor/five-project
and performance observations are still pending, so this checkpoint is not goal
completion. The stock source-policy baseline also answers all eight semantic
requests correctly, including inherited ISO source navigation; its separate
parent-edit extra probe needed an exact-URI fixture correction and rerun.

Latest combined focused check is native-diagnostic-sdk-focus87-3:51 passed,
zero failed/ignored (SDK7, diagnostic contracts41, runtime3), fmt passes. Root
verified aggregate raw counts, every raw hash, stable guards, EOF and zero
residual cleanup. This includes the same-selected-home SDK8 source layout fix
and certified Object hierarchy handling; SDK8 binary Object inventory remains
Unknown. The next full no-fail-fast workspace run (native-editing-sdk-release87-3)
reports1,561 passes, two failures and four ignored tests; it produces no release.
The remaining late-open extension fixtures rely on an unselected synthetic SDK
becoming globally visible, which the new SDK scope correctly forbids. Fix the
fixture to exercise accepted dependency ownership and actual model scope change,
retaining unknown-to-known results, stale-generation rejection and open-buffer
truth. Then rerun its target and full workspace/release. Native Java345, CEM18,
live reload/KAPT/KSP and stock source-policy comparison await the immutable copy.
The shared private Gradle home is free after the successful configuration-only
Gradle help bootstrap (native-five-gradle-bootstrap86-1; no full project build).
Grade has the CEM21 then reload/processor17 queue; Five captures the fifth model
on the new binary after those owned stages close. Core nominal reference work
starts after successful release-copy, preserving immutable editor provenance.

Latest focused checkpoint87 checks: selected SDK six tests and known-leaf
Java/Kotlin override two tests pass with unchanged semantic assertions. Root
verified raw hashes, test counts, stable guards and EOF. Each test Cargo stage
required one residual child cleanup; this is not proof of clean LSP shutdown.
Evidence: native-selected-sdk-focus87-4 and native-known-leaf-causal87-5.
The known-leaf comparison passes with no ancestor, Base and GenericMid when
fixture-owned nominal return facts are present. No binary-dispatch metadata
change is justified; the previous unindexed SDK List case remains an input
coverage gap. Loads are labelled initial/repeated, not an unproven warm hit.
The first combined workspace attempt stops at697 passes, two failures and two
ignored tests; no new release exists. One old test incorrectly expected a
synthetic host SDK for a model with no certified SDK. The other uses a persistent
running marker that may survive legitimate subprocess cancellation. Exact
contract/liveness test corrections and a fresh combined check are pending.
Evidence: native-editing-sdk-release87-1/receipt.json.

Latest published runtime: native-binary-shadow-release86-3/bin/ktlsp.exe
(cf9ebc7655eb2450b9168181e5954cd3edc651d14ead4bcda1a7afa154a3bb78).
Checkpoint87 SDK, diagnostics and source-query changes await the combined release.
The focused selected-SDK suite is assigned to UX, the sole current Cargo owner.
Core is checking a no-binary-ancestor positive control before attributing return
inference failures to binary hierarchy metadata; no cache-version change yet.
Five owns the task-private Gradle home for its final real model capture; four
project models are accepted, but the unchanged original50 editor probes await
this release. The last Gradle capture closed without a model because a private
Gradle dependency-accessor cache lacked metadata. Its closed receipt is
native-five-gradle-current-model86-2/receipt.json; a bounded retained repair and
one new capture are authorized. This is an operational blocker, not a semantic
pass or failure. Grade runs actual reload and offline KAPT/KSP editor checks
after Five releases the shared home and the new immutable binary is available.
Reviewer is assigned the installed IDEA server oracle in an isolated home.
These are assignments; agent API running does not establish active execution.
Root independently audited native-gradle-parent-guards87-1/receipt.json: seven
malformed/unsupported parent cases preserve the root source records and fail
POM certification without discovering a parent. Both native stages exit zero
with EOF, no residual termination, stable inputs and matching raw-log hashes.
This verifies model guards, not the portable fixture's pending editor behavior.
No preexisting process adoption/global process inventory. Keep AGENTS.md untouched.
