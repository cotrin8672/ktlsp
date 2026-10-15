# Current fork state

Read this before the historical acceptance record. Goal remains in progress.
Updated2026-10-15 (Asia/Tokyo), checkpoint86 base verified; goal incomplete.

Latest verified checkpoint86 result: fmt, the serialized-symbol wire regression,
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
Launch state: current-slice84.json; no native task active.
Proofs: native-k2-java-release83-1; native-sdk21-public83-3;
native-sdk21-collector83-1; root-cem-model13-audit83-1/audit.json;
native-cem-java-floor83-1/receipt.json (diagnostic coverage gap).
Working runtime: native-k2-java-release85-1/bin/ktlsp.exe (fe83f1ab...).
Published/native-proven runtime: native-k2-java-release83-1/bin/ktlsp.exe (e1740b2a...).
Latest closed private Gradle-home owner: native-cem-java-floor84-1/receipt.json.
Both stages exit1 for original semantic mismatches, EOF/owned0/unforced, stable
guards. controller_error and final_guard_error are null; these prove completed
observation and closed ownership, not a passing semantic floor.
No preexisting process adoption/global process inventory. Keep AGENTS.md untouched.
