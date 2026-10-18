# Current fork state

Updated 2026-10-18 (Asia/Tokyo). Goal active; IDEA parity and speed acceptance remain unproved.
History: docs/fork-acceptance.md, docs/fork-corpus.md and Git.
BASE denotes C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1.
Other evidence paths below are relative to RUN:
C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda.

## Current93 verification in progress

The narrow SDK21/target17 InProcess compiler profile now passes the40-test
compiler group after its new captured-profile regression first failed. Other
execution strategies and opaque daemon arguments remain rejected. This is not
yet a passing native CEM reload86 run.

Source enrichment now compares unique modules by build/project identity while
preserving every other model fact and all inner ordering. The four source-snapshot
tests and the retained actual194-module pair pass after reproducing the failure
before the fix. Duplicate identities and binary/compiler/build-file drift remain
rejected. A fresh normal Gradle import with this binary is still unrun.

The Gradle exporter reuses its securely configured XML factory while reading
fresh bounded POM bytes and creating a new builder per document. Extracted actual
exporter checks cover37 real POMs and14 invalid/security/fresh-byte cases. Nine
paired warm333-call samples improve the median from360.595ms to229.405ms; these
are function measurements, not whole-project startup or p95 acceptance.

Five optional startup trace types attribute JDK planning/parsing/cache-store/
publication and jar resolution. Three focused LSP tests pass; a target-project
startup comparison has not been collected. Formatting and the complete93 workspace
suite pass:1,616 passed/zero failed/five ignored across40 groups. Root verified
the raw artifact hash, counts, exit0/EOF/live0/residual0 and438.578s closure.
The additional ignored case is the separately passing retained194-module
source-order replay. Workspace receipt: native-integrated93-1/workspace-receipt.json,
SHA2567dfb9d017a57aa496aa11f447d6ec14efacf326c2a6517bacc66a2b63151f394.
Release93 also passes (431.187s build observation). Root verified all311 current
build-input hashes, five artifact refs and the immutable19,691,008-byte copy:
native-integrated93-1/bin/ktlsp.exe SHA256
0a85b4e1a09af2865a3f67c51612e8bdb011485aa2e8e1bdee69d124e3d433d3.
Main receipt: native-integrated93-1/main-ready.json SHA256
d88784cd4250f943ba957277a7b544400c5cbc7c73dc5a68b5e497c9ecb6d334.
Actual Neovim basic and semantic harness runs on this copy return exit0/ALL PASS:
nine and137 assertions respectively, not146 independent requests. The semantic
run verifies graceful shutdown; basic reports its forced client cleanup. Evidence:
root-native93-{basic,semantic}/artifacts/{summary.txt,basic.out,semantic.out},
using each scenario's matching output file. Runs overlap other work and do not
establish latency distributions. The separate owned Java harness passes367
assertions and its graceful-shutdown check on the same copy; Root verified all28
bound artifact refs. Receipt: recovery89/cem90/editor-smoke93-1/receipt.json,
SHA25607bda16e8cfde90f79f40e503a4cd32bf51c3b31e35fce18e3aadd8bc596bc05.
That existing Java trace emits all five new span types (plan1, parse-shard8,
symcache-store1, publication1, jar-resolution1); no extra measurement run was
required. External-extension tests also pass4/4; its release build is still
running with the single Cargo owner and CARGO_BUILD_JOBS=1.
Focused evidence: recovery89/cem90/focus93-summary.json,
SHA256e6ead20f7b26748be641fa5826b868b750d78c7cac53224ef8921e8590a0a439.

The processor114 failures are now bound to an absent current Java diagnostic
store: the active KAPT/KSP bridge was not admitted, rather than a compiler run
certifying the obsolete calls. An independent compiler control produces all
three expected missing-method errors. Current processor-input/output ownership
is still missing; saved generated files alone must not qualify dirty inputs.

The genuine external-library run passed20 new source-navigation/descriptor/
diagnostic checks but retained a baseline binding failure. Closed trace comparison
shows that failing completion finished before the selected Shadow.java source
was materialized; the first responses succeed in a separate warm diagnostic run.
The scoped unit regression passes and does not reproduce this readiness race.
No production fix or stable acceptance is claimed. Exact hydration publication
timing is not traced. Evidence: recovery89/extension93-closed-readiness-comparison.json.

## Verified91 baseline

The coherent native-integrated91-1 workspace
run passed1,608 tests, zero failed/four ignored across40 groups. Root independently
verified raw bytes/hash, counts and exit0/EOF/live0/residual0. Raw SHA256
9becc94a5e773caa11e4c2791c80a72f90772f2066c521c4e3f7b258b90700b1.
The retained194-module protocol fixture is normally ignored but passed explicitly
in focus5; the other ignored entries are the owned subprocess fixture, optional
Gradle sample integration and Maven-download test. Formatting, release and the
immutable copy also passed. Root independently verified all251 current build-input
hashes, all three raw stage hashes/closures and the19,640,320-byte copied binary.
Receipt: native-integrated91-1/receipt.json SHA256
71520dba8256c3949b80685b056b059a3fb0bf1407c0d59236f8ba78e7c80e2e;
bin/ktlsp.exe SHA256c473e3459806a662456a178cf7c60802fbe50b6aff6e9a0d17503ca653f677c0.
Native91 editor validation has exposed the remaining gaps below; full acceptance
remains unproved. The build receipt records the inputs at build/copy time, rather
than certifying subsequent92 source changes.
Focused evidence verified by Root:

- Shared String caller identity: full member-selection65/65, including exact JVM
  String mapping, nullable/generic returns and foreign same-name negative controls.
- Reverse hierarchy filters borrowed candidates before cloning/sorting matches,
  preserving module/parent identity and deterministic order. Hierarchy15/15 and
  member-selection65/65 pass. Spring's actual17-second rename needs a new native
  measurement and applied-edit inspection before claiming a speedup.
- Closed Gradle output is separated from owned-daemon cleanup only for model
  queries; compiler receipts remain strict. Both real Windows process controls
  and the retained109MB/194-module strict protocol replay pass. Captured compiler
  profiles include actual Gradle9.6.1/SDK21/target17/defaultUTF8. The later actual
  import and native generic outcomes are recorded below.
- Demanded archive-source hydration passes7/7, using the real consumer contract,
  exact selected archive/symbol identities, stale/configuration/unsaved controls,
  failed-load deduplication and preserved existing/conflicting ownership. The
  ownership negative allows only the independently valid Item source; it still
  rejects Factory certification and binding. Actual external-library22 native
  checks remain pending.

Evidence: binary-generic91-focus5/receipt.json SHA256
d4e05cca915f4e27b65aeabda9d2f09f2ac7786ad61085e0816ef54d0bc73112
(246 passing executions/five initial fixture failures, retained);
binary-generic91-finalfocus1/receipt.json SHA256
509c1a4b2413c3802a861b420f7ab68d194286a4eeab19a025bf17309928367f
(hierarchy15/member-selection65 pass; one ownership-test expectation failed);
binary-generic91-hydration-final1/receipt.json SHA256
4a5f41ffc040536577cee021ef004507c327b64f6f41ed7a7cb31af7f36414f1
(all seven hydration checks pass after preserving the independent Item demand).

## Current92 fixes and actual91 editor results

The coherent native-integrated92-3 checkpoint passes formatting, the complete
workspace suite (1,612 passed/zero failed/four ignored across40 groups), and the
release build. Root independently verified all256 current build-input hashes,
all three raw stage hashes/clean closures and the immutable19,684,352-byte binary.
Receipt: native-integrated92-3/receipt.json SHA256
9eb5d983ef3ce9ff705fc8df61a769d96ce9162be08e29d49ac56cf81606fe1a;
bin/ktlsp.exe SHA256
442fe755ae952a706f6558ceb51e8b9e95d5b2d7a0a3e9798f16e2d01de8b726.
Actual unchanged native32 validation now passes32/32 on this copy, including both
previous Java recovery failures. Root verified all four receipt artifact refs,
the24 ordinary first-response records and eight bound raw extra responses. This
is one correctness run, not a performance distribution. Evidence:
binary-generic92-native-editor/receipt.json SHA256
83e3d5c941e6248aa791bb7405ecb675a9b72b156d0ada11c0c215c972cd83f8;
report SHA2568f783b00d637230e02d1733232308ec7b313974221ebe57c8e82128cc125c9f2.

The initial92 full suite closed with1,611 passed/one failed/four ignored across40
groups; Root verified both raw hashes, counts and clean subprocess closure.
The sole failure is the watch/coalescing fixture's second-startup assertion
(observed invocation count1, expected2). Its three independent waits reuse one
12-second deadline; actual elapsed phase times were not captured, so expiration
is not yet a measured cause. Preserve the failure while repairing/checking the
wait fixture; release/copy did not run. Evidence: native-integrated92-1/receipt.json
SHA25629fa082a7297a4862bf19c7961df2bb89c82cb4174a2e9a686770ea25f5b26a6.
The test-only repair gives each subsequent independent wait its own original
12-second observation bound. Counts1→2→3, exclusive child lease, no overlap and
latest-generation publication assertions are unchanged; no production timeout
or performance expectation changed. The actual-child focused test passes once
(Root verified both artifact hashes and clean closure). Evidence:
binary-generic92-scheduler-green1/receipt.json SHA256
601b378190a23ef2a83912a5ab84cd2e6b7cfa53f426b1876c1f2f327ac72eaf.
Attempt92-2 stopped before any tests with an LLVM out-of-memory build error.
The successful92-3 run sets CARGO_BUILD_JOBS=1, with no Rust/profile or product
timeout change. Its workspace run takes135.562s and release619.484s; these are
build observations, not LSP request measurements. All four ignored cases have
the same roles as the91 baseline; no passing total includes them.

- All50 original five-project requests have now received actual first responses.
  Gradle's remaining9 return four raw matches/five mismatches; its three ordinary
  cross-language class/interface requests pass, while five AtomicInteger member
  positives fail in saved/partial/dangling/broken-peer/control states. The matched
  unknown-receiver negative lacks its required useful AtomicInteger control and
  is unqualified. Thus the aggregate is45 raw matches/five mismatches, or44
  qualified matches/one unqualified negative/five missing positives; none of this
  certifies all five projects. These results span immutable90/91 binaries, rather
  than one latest92 replay. Root verified both raw stages, both reports and
  binary hashes, nine observed requests and zero operation failures. Evidence:
  recovery89/corpus90/native-five-gradle-editor91-1/receipt.json SHA256
  fe4757b9b2d220d2e3ba49dd248beb67bba82e31fb45dfaf6d6953c366cc711d.
  The source-enrichment query completes normally but its model-consistency check
  rejects the result. Read-only comparison finds identical module facts/compiler
  records/build mtimes and only differing outer PROJECT order. Test-only Rust
  execution now reproduces the failure on both a small case and the actual194
  module pair; the93 production fix and focused green checks are recorded above.
  Actual KAPT/KSP114 validation closes with111 passed/three measured failures.
  The failures are missing Java diagnostics for obsolete generated APIs from
  KspKotlin, KspJava and KaptJava. Compiler generation/builds, source navigation,
  disabled/restored states and dirty Kotlin completion are exercised separately;
  no complete plugin acceptance follows from111 passing assertions. The exact
  Java accepted-store/bridge cause is bound in the93 findings above. Evidence:
  recovery89/corpus90/native-processor114-editor91-1/receipt.json SHA256
  bf6ad48e925bf3a66131a8933c81f163741572de809cf8ba7dc759e39c99d169.
- The actual source-less generic fixture returned30/32 matching first responses:
  all original18 pass, with one mismatched and one unanswered extra. Both failures
  affect an earlier known-instance Java call when a later statement is unfinished.
  The exact original buffer first passes its useful baseline, then reproduces both
  contexts in Rust. Java completion-prefix recovery previously accepted only a
  bare identifier receiver. The small fix preserves verified receiver call chains
  and the existing binding/namespace checks. Root verified all three focused raw
  hashes and clean closures: Java frontend27, member-selection65 and generic125
  pass (217 total). Evidence: binary-generic92-green2/receipt.json SHA256
  e2fc2e4cc46f2ef1da3d55a39acbe1f66e801bb2d906edc79d7f1a318ed50b97.
  The new release and unchanged native32 replay pass; the old30/32 failure is
  retained for comparison rather than relabeled successful.
- Spring's actual91 unsaved-edit probe passes all16 first-attempt requests and17
  semantic assertions. Completion, exact navigation, references, applied rename,
  restored rename and API retraction are useful. Returned rename edits were
  independently applied to both complete buffers and match their exact expected
  contents. Root verified all seven bound input hashes. The member-family phase
  takes2.103720s in this single sample; the response arrives within the unchanged
  five-second RPC limit. This does not establish p50/p95 or a speed target.
  The original owner receipt retains its failed combined guard: only the mutable
  genuine Gradle model changed through normal server refresh; immutable inputs
  remain unchanged. Shutdown required termination, so operational acceptance
  remains open. Evidence: recovery89/corpus90/native-five-spring-rename91-1/
  semantic-receipt.json SHA256
  fa194be805f3d78ce179cf6ca8cd4697197baa283e940d092a22401c1ad1cc61.
- Automatic Gradle reload86 is not accepted:70 checks pass, nine assertions
  mismatch, one canonical Kotlin diagnostic is unmeasured and six removal
  controls lack their prerequisite positive. All four actual offline builds
  succeed, all10 model metadata checks pass and dirty helper buffers survive.
  However, none of the three refreshed index generations finishes within the
  existing observation window. Stale completion/source-navigation responses are
  recorded as returned; they do not establish a failure after a ready generation.
  Separately, the actual SDK21/in-process Kotlin profile was unsupported by that91
  Java header-diagnostic adapter; the93 focused admission fix is recorded above.
  Do not retarget the SDK or increase the
  window to hide either gap. Root verified19 referenced artifacts. Evidence:
  recovery89/cem90/gradle-reload91-summary.json SHA256
  47c611d4c82902ce48bd5356546fb213f826a5250e8cd4476575632abdc35865.
  A separate actual SDK21/Kotlin2.4/target17 in-process control passes the exact
  clean-output Java path: two useful positives, stale-member/type/import negatives,
  and the preceding Kotlin/Java bridge compilations. Root verified13 artifact refs,
  exits0000111 and the three expected diagnostic codes. This supports a narrow
  admission fix; dirty Kotlin header support and refresh speed remain unproved.
  Evidence: recovery89/cem90/sdk21-inprocess-control91-1/receipt.json SHA256
  eefff0b8bfac477e573d4b801516ecdaa291aee6a8c09cc516287eeda1bc008f.
- External extension91 retains all original50 useful results and38/40 shape
  results. Remaining Any definition/valid-diagnostic failures have a concrete
  shared-core cause: the real2.1.20 stdlib source archive contains common expect
  and JVM actual declarations; extension type lookup rejects the duplicate before
  demanding source. A focused fix reuses ordinary source-set selection only for
  one verified archive/origin, retaining genuine ambiguity. Test-only reproduction
  first passed three controls and failed the actual/JVM positive. After the shared
  fix, all43 extension-core tests pass, including the four new cases. Root verified
  both bound artifact hashes and clean closure. Evidence:
  binary-generic92-kmp-green2/receipt.json SHA256
  c99b22799b84fcdb1be4484f4a4c23964dffe059f12a35860d7a2e8ee4e8b9cb.
  The new full suite/release passes; native Any checks remain pending.

## Native90 evidence

Earlier published checkpoint499e98ec8bbd11ee66b44e6a98c8cdc8b68fd187 is on
codex/java-kotlin-analysis; its remote hash was verified. Results below for
snapshot89-6 apply to that checkpoint, rather than the later90 changes.

Latest complete acceptance remains unproved. Moshi90-2 exported genuine MODEL14
through the normal server (11 modules), then the unchanged original9 requests
passed16 semantic checks in real Neovim, first-response/single-attempt. Both editor
stages closed exit0/EOF/live0/residual0; Root independently verified raw SHA256s and
request summary. Evidence: recovery89/corpus/runner-snapshots/native-five-moshi-editor90-1/receipt.json
(SHA2561df66e7cc9747c6cd38cfc03aa1e6216649bc0cdbb50a0511f6b1240bc8c819d).
Separate14 unsaved extras plus2 controls closed:13 matched,2 mismatched,1
unmeasured. Return chains substring/incrementAndGet lacked useful completion;
member completion/exact goto/references/applied-and-inspected rename and API
retraction/restoration pass. Counter diagnostics had no accepted compiler receipt
with the incomplete selected Kotlin recipe, so empty output is not certified.
Evidence: recovery89/corpus/runner-snapshots/native-five-moshi-shared-editor90-1/receipt.json
(SHA256da6874c554999ca3b83e0540de13e4ebed239efeb32dd514800a042df9ee05f4).
Causal receiver probes continue; do not assume the SDK gap is the cause.
The later actual :moshi:jar build and normal model refresh completed both selected
language tasks; unchanged originals again passed9/9. Unsaved cross-module canonical
diagnostics still stop at dirty-visible-dependency-source-unhandled. The producer
is mixed Java/Kotlin and its selected jar also carries Java16 multi-release entries;
replacing it with saved or Java-only class output would lose actual semantics.
Preserve this guard until a validated producer compilation and archive projection
cover the current unsaved sources. This is separate from the String caller-view
fix and missing AtomicInteger API inventory.
A missing producer build/libs jar leaves a Kotlin compiler task incomplete, so
request9 success does not establish full compiler-view coverage. Other original
counts: OkHttp10, Retrofit12, Spring10, Gradle9 (50 total across five projects).
The first Moshi run had successful Gradle root exit but a surviving descendant
exceeded native runtime grace, forcing cleanup and discarding the model. A later
normal fallback closed naturally and accepted the model. Treat this as an
intermittent import-lifecycle gap; remaining child identity is not yet established.
Do not certify an empty model as successful initialization or manually inject one.
Original five-project requests now41/50 measured matched: Moshi9 + OkHttp10 +
Spring10 + Retrofit12. This is the original request subset, not full acceptance of four projects.
Root independently checked OkHttp receipt summary/hash and both raw hashes/clean
stages: native-five-okhttp-editor90-1/receipt.json
(SHA256a937fea666b01997de81a253ed97e62372d9cc8a0f8c020a5e1be47cd5dade04).
OkHttp separate16 extras:10 matched/2 mismatched/4 unmeasured, including String
and AtomicInteger return chains, unsupported Java diagnostic recipe and rename
observation gaps. No full project acceptance from the original-request subset.
Retrofit recovered the actual incomplete Gradle8.7 cache entries and then exported
a genuine26-module MODEL14 through normal import. Both unchanged editor scenarios
passed all12 original requests on single first responses, with clean shutdown.
Root verified both receipts, all three raw hashes/bytes/exit0/EOF/live0/residual0,
and the actual request outcomes. Evidence: recovery89/corpus90/
native-five-retrofit-model90-4/receipt.json SHA256
3b63cc5a6733b9b8bd378ed3cee6a72c1baed559746edcd5d438cfd22a483b15;
native-five-retrofit-editor90-4/receipt.json SHA256
125cee6186f8064dc4338b9a231ec33d70b83dca37018482d9c5798c9d6302e8.
Its Java8 task recipe remains unsupported; these semantic requests do not certify
canonical diagnostics or full dependency indexing (four missing source archives).
Spring exported a
genuine26-module MODEL14 and completed project/library/JDK indexing. Its editor10
all matched on first responses; Root verified both raw hashes, stable guards,
request count/attempts and actual disk-cache model_load/source_install trace.
Receipt: recovery89/corpus90/native-five-spring-editor90-2/receipt.json, SHA256
ed59658f3e8883000110071955acd7896e1c421f69b5a6114fb6ae47f5b60069.
Normal source-query/model_store events updated that private cache after startup;
the pre-editor e0b3... model hash and post-server ed293... hash are distinct states,
not a promise of byte identity or a manually injected model.
Golden shutdown was clean; the incomplete-code run needed force, a separate gap that
does not invalidate the genuine model or justify repeating its import. Gradle's
normal online import closed after1,587.297s: actual Gradle root exited0 and both
readers finished, but the surviving owned GradleDaemon50304 exceeded2s cleanup
grace and the current server discarded the dump. Native identity/timeline evidence
isolates that daemon; this is not a Gradle build failure. Full protocol validation
and a semantic-result/cleanup-policy fix are pending; no accepted fifth model is
claimed. Evidence: recovery89/corpus90/gradle-runtime-tail90.json.
Actual official :moshi:jar build passed without source edits; new normal model and
diagnostics remain required. Selected Moshi stdlib1.9.21 is explicitly in Gradle
binary/source artifact selection; compiler1.8.21 does not imply a provenance bug.

CEM90-2 no-adjacent-source input run closed all18: original10 had3 matched and7
mismatched; aux8 had4 matched and4 unmeasured. The real generated adjacent source
archive was absent from the recovered snapshot, although it exists in the real
project. Existing product discovery and invalidation already support this exact
adjacency; do not add duplicate discovery logic or claim a normal discovery bug.
A new qualified snapshot restores the exact real archive with binary/source hashes
and adjacency recorded (recovery89/cem90/generated-source-restoration.json).
Its genuine complete-input Gradle import closed, followed by the unchanged18
editor run. Original10/10 matched on first responses; auxiliary4/8 matched and4
remain unmeasured. Root independently verified all eight owner/result/trace file
hashes, both stage raw hashes, stable closure guards and single-attempt original
results. Evidence: recovery89/cem90/complete-source18-summary.json, SHA256
54a1f45c0809754e5b3781ef77215e99e7bc22e0f4a147675dfa2fe79ba8993c.
Both controllers intentionally exited1 for unmeasured auxiliary cases; owned
EOF/live0/residual0 establish cleanup only. LSP shutdown itself still needed force.
The original10 cover one ordinary non-Mixin Kotlin caller,
EnchantableDrillBlockEntity.kt, with superclass/Holder API queries and incomplete
edit variants; BlockEntityExtension.kt is edited only as peer context. They do not
prove whole-CEM coverage. Auxiliary8 use a Java Mixin file only for ordinary
missing-import/store/restoration checks, with no Mixin semantic claim. Other
ordinary CEM files, unsaved cross-language API propagation, references/applied
rename, broader signatures/nullability/overloads, generated-source invalidation
and project-wide diagnostic precision still need independent validation.
Retain the no-source fallback gap: nested/default generic Holder APIs are not
covered by90 top-level generic projection. Independent actual CEM compiler controls
accept Holder.Reference<Enchantment> and reject wrong String; no expectation change.
Concurrent no-source first-use572.557s, source query302.901s (HTTP metadata wait
observed), binary preview261.735s. These are observations, not speed acceptance.
The complete-input first-use observation was267.176s (455 availability polls,n1,
concurrent load); it is not a first-response acceptance or speed comparison.
Canonical diagnostic phase attribution found58.717s in the first Kotlin compile,
22.316s building the header helper,1.954s building the Java helper, then a second
full Kotlin replay cancelled before Javac started. The120s editor observation
expired and restored the Java buffer; stale-input rejection then correctly
cancelled that older request. No stale-input validation or timeout weakening is
justified. Reusing exact closed canonical Kotlin outputs for Javac is being
reduced and tested before changing the legacy header path. Three controlled
stdio shutdown probes exited naturally; no speculative transport fix is justified.
Removing duplicate compilation alone does not meet interactive performance: the
remaining full Kotlin pass is still52–59s in these concurrent observations. Measure
the new pipeline first, then address remaining startup/analysis work rather than
claiming speed from fewer compiler launches.

Five native gpt-6.1-sol/high agents are active, no new worktrees:
corpus90 fresh imports/editor requests and Java compiler-recipe adapter coverage;
cem90 canonical compiler reuse/performance and remaining CEM diagnostics;
binary90 generic classfiles plus shared Java field/static/incomplete-edit queries;
ux90 unfinished Java completion recovery, then per-caller SDK API-view integration;
extension90 shared reference kinds,
lexical member context and dependency-origin host-fact availability.

Current90 source is unintegrated. Core Signature parser3 and extension39/cache1
focused tests pass. Genuine subsequent REDs exposed Java ordinary generic-field
chains and unqualified nested target signatures. Shared fixes are authored; static
field/incomplete-edit namespace closure and query-wrapper consistency are being
checked. The preceding focused Jar batch passed25 tests and the cache-shadow
test; Root verified receipt/raw hashes and stable guards. A subsequent leaf test
exposed lost arity for a valid nested JVM return descriptor. Raw descriptor arity
is now kept separately from unsupported source-type projection. Bounded callable
facts, unfinished completion/goto recovery, lazy namespace proof and clean K2
canonical-output reuse are authored; their combined focused batch is running.
Combined green8 closed with stable guards: descriptor1/Jar25/cache1/serialization1/
known-leaf2 passed; full generic interop had121 passed and4 failed. One unsafe
Kotlin bounded-return path bypassed the shared solver and is being corrected.
Two prior valid bounded cases need compiler-confirmed positive expectations rather
than the old Unknown coverage-gap assertions. A compiler test filter selected0
tests: it supplies no semantic evidence and must be rerun with the correct
compile::java_diagnostics::tests filter. Root independently verified all seven raw
hashes, stage counts and EOF/live0/residual0; receipt SHA256
fb4c0c5e84827b31cac1be853ac5d4a6d01b1fc5da3bd16416c5ff47aecf114f at
binary-generic90-green8/receipt.json under the run directory.
Subsequent shared-solver fixes retained the existing extension/reified partial
container behavior. Final focused green10 passed all125 generic interop cases and
the external source-shape adapter case; Root verified counts/raw hashes/stable
guards. Receipt SHA256980fa9f8444349df7700a689f6b7d499c66006c20fb2d58fb3ade9683758b2c3.
The correct compiler module independently ran35 tests with zero failures, including
canonical-output and private-wire tamper guards. Actual javac/Kotlin2.4 controls
accept valid bounds and reject Class<Other>; Root verified all four raw hashes and
the actual incompatible-bound errors (javac console output is CP932).
Initial broad run stopped at formatting only: scoped2024 rustfmt differed from
workspace2021 formatting. Cargo fmt --all corrected it; native-integrated90-2 now
closed the full suite at1,587 passed/3 failed/3 ignored (40 groups), before release.
Root verified the raw hash/counts and stable guards. The failures were an old cache
test format integer, lost Java enum-constant navigation, and a scheduler fixture
that could cancel a child between creating its lease and recording its invocation.
The cache test now uses the current format, Java enum constants retain implicit
public/static facts and exact navigation, and the scheduler fixture uses an
explicit second-child handshake while retaining3 invocations/no overlap/generation
checks. Production refresh scheduling was unchanged. Focused green11 passed all
four selected checks; Root verified its raw hashes/counts/stable guards, receipt
SHA25681d358164c3f84d04d424123fa63530e04f7476d0bd940b1a1af179d9f7d3a5f.
The final native-integrated90-3 full workspace stage passed1,590 tests with zero
failures/three ignored across40 groups. Root independently verified raw bytes/hash,
counts, exit0/EOF/live0/residual0; raw SHA256
c7d8aae7bc84ec990ab75cb55077322ce5ae8a63d8576eecdb8a33c001d342c3.
Release and immutable copy also passed; Root verified all250 current input hashes,
all three stage raw hashes/clean closure and the copied19,506,688-byte binary.
Evidence: native-integrated90-3/receipt.json SHA256
cd5eb17a6c0d6cecbb7723c219ed34e8d2a0bb13bab4e6df6b14b3e249283489;
bin/ktlsp.exe SHA25619429b2fbf84719180e6a246ead3ec5847e5b65e20c23e8d2d3f7c95a641a7e3.
Editor validation remains in progress. Java373 first run had372 matches/one probe
assertion mismatch: hidden canonical-prefix completion correctly returned no
result, but the absence-only assertion incorrectly required a non-null result.
All four useful generic-property cases and five enum case pairs passed. The
original failure receipt is preserved. Only negative-case response handling changed;
the replay passed373/373, retaining useful positive-item requirements. Root verified
the actual373 checks/zero failures, all87 SDK/runtime guards, three result hashes,
raw bytes/hash and exit0/EOF/live0/residual0. Evidence:
native-java-generic-editor90-4/java-receipt.json; raw SHA256
45c2bbe465603dbb2eb83070de5dfed32f964a0b2c01b7111847350600bbf09a.
CEM unchanged18 also closed on this immutable copy: original10/10 and auxiliary8/8
matched. The canonical Kotlin class output was reused for Java without a second
full Kotlin pass/header plugin. Root verified the aggregate and six proof references,
both raw hashes/bytes/EOF/live0/residual0. Evidence: recovery89/cem90/
integrated90-4-18-summary.json SHA256
7e7fcd6878781f33f6973cde7cb61c773f8ed942d3db93d526d7a1a3643eaa55.
The primary scenario still needed forced shutdown; the second closed naturally.
Restored-buffer canonical-store readiness remains unmeasured. Combined canonical
Kotlin/Java work took82.767s and56.616s, and first useful completion was observed
at219.822s (one concurrent sample): this does not establish interactive speed acceptance.
Separate ordinary CEM production coverage now passes12/12 first-attempt requests
across EnchantedItemFactory, EnchantableRollerBlock and
EnchantableMillstoneBlockEntity: three completions, three exact definitions,
three hovers and three signatures. Root verified all12 distinct requests and16
file references/hashes, both owned closures and stable source guards. The first
run measured8; four requests were never sent because the probe compared mixed
CRLF/LF buffer bytes with normalized disk text. Exact raw disk/wire equality was
proved, the probe was corrected, and only those four unrequested cases ran next.
One of the two server shutdowns needed force. These are immutable90-3 observations,
not current91 revalidation or general unsaved-edit/latency acceptance. Evidence:
recovery89/cem90/ordinary-production91-summary.json SHA256
4dcee7b2ca2c3af147cecd56686721e1250d38a80ebc81585b1ae92e9e8b9d49.
A separate ordinary Kotlin restoration control now closes that readiness subset:
one final pull returned full[] with the same result ID, source generation and exact
restored source hash as the accepted clean Kotlin2.4.0 store. Actual canonical
compiler execution exited0 with both streams complete in41.849s; shutdown was clean.
Root verified eight proof references, current source bytes, raw closures and store/
response correlation. Evidence: recovery89/cem90/ordinary-restored-canonical91-summary.json
SHA2560d740394cf3ead90f7fa59828edebfa9fc98590ab7270c8cdc37a8c64ced5413.
This is one immutable90-3 ordinary Kotlin control; Java absence authority and
general restoration/lifecycle acceptance remain open.
The copied external extension run retained all original50/50 outcomes exactly,
but added shape coverage passed37/40: Kotlin Any definition, completion and valid
binding diagnostics failed. Root independently compared all90 single-attempt
outcomes and the original50 with89-3;141 semantic checks matched/three mismatched,
with graceful shutdown. Evidence: BASE/external-shapes90-1/receipt.json SHA256
601b63e4bc215df17a1293fb22b8fe59c7cce7b297596c52107a528ba84cec02.
Demanded library-source hydration is the current91 fix under validation, not a
passing native result yet. Its origin fallback must preserve existing project
ownership and conflicting library origins.
The binary generic fixture retains its original target17/default-encoding
consumer on SDK21: this exposed an unsupported compiler-recipe profile, requiring
an actual capture/adapter fix rather than retargeting the fixture to21 for green.
Source and binary generic
coverage includes nominal method bounds and same-name method parameter shadowing;
inter-variable bounds and parameterized nested owner types remain Unknown and are
valid-code coverage gaps, not acceptance successes. UX SDK patch/tests and extension demanded-library
hydration are drafted outside RootRust during guarded builds, then must merge as
scoped hunks after the coherent90 build/copy. Do not overwrite concurrent edits.
Unrun native shape40, Gradle reload86, processor114 and parent-edit5 remain pending.

Semantic exit0/assertions/stable inputs remain evidence when exact-owned tail
cleanup is needed; record lifecycle cleanup separately and do not repeat tests
solely to obtain zero residuals. Keep bounded/cancelable exact-owned cleanup; no
preexisting-process kills. Cache reuse is allowed for sequential correctness runs
with genuine input/revision checks; do not impose cold reindexing on every case.

## Latest integrated snapshot89-6

Formatting, full workspace --no-fail-fast (1,573 passed, zero failed, three ignored)
and release pass. Root independently verified all three stage logs/hashes, clean
EOF/live0/no residual termination, binary bytes/hash and all199 current source guards.
Evidence: C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda/native-integrated89-6/receipt.json.
Immutable bin/ktlsp.exe is19,407,360bytes; SHA256
7489fea6a1c0cfc062bb07beb2176d5a527d4b6b13fe6b3fd078de7c8227729c.
Actual Neovim Java353 passes with this copy, including valid/missing/restored Kotlin
property first notifications (versions11/12/13), real default stdlib2.1.20 and graceful
shutdown. Root verified353 raw PASS assertions, zero FAIL, owned raw hashes/clean
exit and all six runtime guards. Evidence: BASE/native-java-stdlib-editor89-1/java-receipt.json
and acceptance-summary.json. External fresh linked executable also passes unchanged
50 single-attempt requests/98 checks with formal postbuild and full SDK/runtime guards,
graceful shutdown and six clean owned stages. Root verified all stage raw hashes.
Evidence: BASE/external-nominal89-3/receipt.json and verified-summary.json.
Gradle reload89-1 stopped before LSP on missing task-cache stdlib2.4; no semantic
acceptance. Grade restores exact verified artifact before fresh89-2 and processor114.
External build/copy is closed and checkpoint499e98e is published. Next source
slices and Gradle editors use distinct guarded inputs. Goal stays active.

## Previous published checkpoint

b9935b17b46fce007c4f05687e9e94c5d660f8e0 on codex/java-kotlin-analysis.
Root verified fmt, full workspace (1563 passed, zero failed, four ignored), release,
raw hashes and clean closure. Evidence: BASE/native-editing-sdk-release87-4/receipt.json.
Server SHA256 d1d133307da739327d6b6cbf08bfd3b16e64ea2fb176dd58427c81b897b31d61.
Actual Neovim Java346/346 includes strict graceful shutdown, six annotation first
responses and two actually applied imports with javac controls.
Evidence: BASE/native-java-editing-editor87-2/receipt.json.
Fresh CEM original10 plus auxiliary8 passed. Evidence: BASE/native-cem-first-use-editor87-1.
First useful completion151832.564ms during concurrent work; binary preview27494files/
488644symbols in44.205s and source query101.533s. Speed acceptance remains a gap.
CEM89 requires a fresh real model: old model bytes are missing and the current real
NeoForge21.1.248 jar differs from the old captured hash.39/40 missing snapshot inputs
recover exactly; preserve changed-jar provenance. Do not strip enrichment fields from
an old model and call it fresh. Keep original10+8 expectations; no user source writes.

## Binary analysis and source-less navigation boundary

User decision2026-10-18: reuse an existing decompiler for source-less implementation
viewing; do not build a decompiler into the LSP. Completion and type analysis read
classfile declaration metadata directly. Decompilation is an on-demand navigation
adapter concern, with tool selection/integration still unverified. Declaration-only
API projection is not proof that method-body browsing is implemented.

## Integrated changes

- Protected access: binary flags, exact lexical caller, Java package access,
  subclass/receiver and validated super shared by completion/navigation/diagnostics.
  Focused144 source/binary/local/module/cold/verified-cache combinations, three native
  Kotlin controls and actual Create getBreakSpeed()F pass. Nested outer access is
  conservative. Cache global29->30, local/module5->6. Integrated suite pending.
  Follow-up nested-call RED exposed outer super privilege leaking into argument
  receivers; selector-only spans fix the160-case matrix. Added actual Create return
  assertion exposed a separate Kotlin inference gap (raw Java float instead of Float),
  now repaired with Java-keyword TypeRef provenance, preserving nominal lowercase
  types and unresolved Kotlin references.40 Kotlin scalar contexts/eight Java raw
  signatures and nominal/unknown controls pass. Independent all8 typed compiler controls
  pass with Float-as-String rejected: recovery89/grade/primitive-oracle89-1.
- Nominal extension identities: opaque host references, canonical nested/reference
  types/arrays and stale/ambiguous refusal. Separate real-Mixin-fixture executable
  passes original2 + primitive14 + nominal34 =50 single-attempt requests,98 checks,
  actual Neovim and graceful shutdown. BASE/external-nominal89-2/verified-summary.json.
  Prior89-1 build lacks an explicit final RootRust hash assertion; runtime/raw guards
  are proved but that postbuild guard is not. Fresh integrated confirmation pending.
  Production Mixin support belongs in a separate project; this is a test extension.
  Nominal issuance currently supports ordinary class/interface shapes only;
  enum/object/record and generic/value-class shapes remain Unknown. In particular,
  this does not yet establish enum-valued Minecraft Direction.Axis signatures.
  Follow-up genuine REDs fixed signature equality (same JVM descriptor, different
  reference provenance) and duplicate compiled contributions; provenance validation
  remains in require(). Repeated nominal reads measured192 full target parses before,
  zero after; retained source upsert adds one eligibility parse.32 core extension tests,
  one parse/invalidation test and one corrected private-parent test pass. Root verified
  raw hashes/exit0/EOF/live0 and stable guards: BASE/nominal-combined89-1/receipt.json.
- Artifact source retirement removes origin before index removal, avoiding an
  authored-deletion tombstone masking valid binary fallback. Focused continuity and
  strict authored-deletion controls pass; actual parent-edit rerun pending.
- Gradle reload harness waits for current accepted indexing generation independently
  of expected goto. Previous87 run78/86: stale Java/shutdown fixed, source probes early.
  Fresh86 checks pending.
- Processor: XML external-access property official URIs fix Gradle7.4 legacy XML API
  shadowing. Offline model3projects/6source sets/16complete compiler tasks and actual
  offline clean :app:classes pass. KAPT/KSP114 editing checks still pending.
  Disabled javac control output is UTF8; no expectation/locale weakening.
- Standalone javaClass false diagnostic reproduced with actual editor and compiler
  control. Reuse existing product stdlib default2.1.20; Gradle retains its declared
  classpath. Generic extension prefix/alias/shadow/nullable/absent-stdlib tests pass.
  Actual353 combined editor passes. Prefix queries use the requested name prefix;
  Java queries skip Kotlin extensions entirely. Current90 Kotlin receiver extension
  queries use the existing indexed receiver buckets instead of a full-symbol scan.
  Ambiguous imported generic properties now have compiler control: same-return
  selection retains both targets and a consensus type, different returns stay Unknown.
  Focused ambiguity/alias/shadow/nullability and bounded Java lookup tests pass.

## Five projects and SDK API views

Pinned OkHttp4.12.0, Moshi1.15.2, Retrofit2.11.0, Spring6.2.0 and Gradle label8.14.3
(actual wrapper8.14.2). Original10 manifests/50 requests unchanged.
All pinned tracked inputs/Git metadata recovered. Fresh normal models now support
the41 original requests on four projects above. The fifth model/latest50 remain
pending the Gradle closed-output/owned-daemon policy fix and actual reimport.
Never adjust cache timestamps to fake freshness.
Extra unsaved edits cover cross-language completion, exact goto, applied rename and
API retraction/restoration, separately from original50.
Moshi/Spring compiler snapshot controls pass. OkHttp six positive closures retained;
two retraction controls corrected localized diagnostic matching with317producer class
hashes identical. BASE/native-five-oracle-okhttp-negative89-1/receipt.json.
Editor extras retain real gaps: Moshi String/AtomicInteger chains and dirty producer
diagnostics; OkHttp chains/diagnostic recipes; Spring rename blocking later requests.
Retrofit extras and the Gradle compiler snapshot control remain pending. Unsupported
compiler recipes cannot certify diagnostics from empty output.

Moshi Java SDK17 --release8 versus Kotlin SDK17 jvmTarget1.8/no-Xjdk-release needs
distinct API views. Independent Java and actual Moshi Kotlin1.8.21 controls confirm.
The actual post-jar model has complete tasks but the common SDK selection rejects
the release8/full17 combination, leaving zero JDK inventory. The earlier incomplete
producer model also failed the metadata guard. A separate real SDK17 archive control
passes8/8 first RPCs and15 assertions for Java/Kotlin AtomicInteger parameters and
returns plus wrong-receiver negatives. Root verified the actual report/raw hashes,
clean ownership and admission trace (13,891 JDK files/285,542 symbols). Thus the API
is present and works when admitted; the project selection must be fixed. Evidence:
recovery89/atomic-inventory91/native-real-sdk17-control1/semantic-receipt.json,
report SHA256ba5489158185ed3624ba3df0eb55d5127c6be45c8f30f7bf51ae7617fd03d422.
Implementation pending; ct.sym signatures cannot directly use current source-backed
stub extractor without generic/signature handling.

The latest official-plugin source-policy rerun covers project imports and8 requests:
seven expected; ISO cafe goto chose binary class. Daemon SDK22 was observed despite
requested21; project SDK is unproved; shutdown needed cleanup. This does not replace
the older checkpoint43 Retrofit21-RPC/25-check comparison or checkpoint60 CEM31-cycle
completion comparison in fork-acceptance.md. Those are revision-specific focused
observations, not revalidation of the current90/91 source or whole-project parity.

## Active work and process rules

Five native agents use gpt-6.1-sol/high; no new worktrees.
Earlier89-3/4/5 integration failures were repaired before verified89-6.
Current roles and unverified90 changes are recorded above. Concurrent correctness
jobs do not establish isolated timing acceptance; private Gradle-home writes have
one explicit owner, with CEM using its own separate home.

Recovered owned69 supervisor is outside Temp:
C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda/recovery89.
Source SHA25658076bd36c926e3bd3294a955cc341518b936581445ee2d26d0c4bd6050c573c;
pyc SHA2562a21d1f65edfe7d338a7072dc6ce97bf56f9d3023c9d52666fc9c83573bac068.
New recovered binding is explicit; clean success/timeout-cancel controls passed.
Keep raw logs on disk and report compact status; distinguish requests/assertions.

Root owns review/docs/selective commits/pushes. Never stage/change user AGENTS.md
(original SHA256553791c5e93098fdb65c3e0104c52febbb15f5773ece17bd0b50b3378743a380)
or generated rtk/history.db. Full objective includes mixed editing, new deps/plugins,
five projects, CEM and measured release speed; no smaller passing subset completes it.
