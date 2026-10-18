# Current fork state

Updated 2026-10-18 (Asia/Tokyo). Goal active; IDEA parity and speed acceptance remain unproved.
History: docs/fork-acceptance.md, docs/fork-corpus.md and Git.
BASE denotes C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1.
Other evidence paths below are relative to RUN:
C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda.

## Latest110: selected KAPT runtime and warning-free Gradle import

KAPT's null java_home is no longer confused with an unselected worker JVM.
The Gradle exporter captures the stock worker executable, build JVM and worker
arguments; the processor plan binds them to the selected runtime and retains
the existing KSP/stubs/toolchain checks. Missing or different runtime facts and
extra worker arguments remain unavailable. The existing focused fixture now
uses the real KAPT model shape: it fails before the fix and passes afterward.
All18 processor-focused checks pass, with one native observer ignored.
Unsupported plan reasons are also available through the existing debug log.

A real Gradle8.14.2 import independently exposes an unchecked-map cast in the
new KSP exporter: this project treats Kotlin DSL warnings as errors. The
exporter now constructs a typed map directly, preserving its serialized fields.
The failed allocator warmup has zero measured samples and is retained under
recovery89/allocator-abba1; it supplies no evidence of a performance benefit.

After both fixes, final workspace passes1,706/zero failed/eight ignored across
40 result blocks; release succeeds. Root checks all348 current inputs and
three referenced artifacts. All final Jobs close without live/terminated
residuals. Earlier evidence retains one descendant-cleanup wall-time failure
(6.031s under load); the unchanged single test and subsequent broad runs pass.
Evidence: native-integrated110-1/receipt.json SHA256
011c5867352d9f529cc685cca66c77c6fa64a81ecf70755a8c135f371775b8ec.
The20,806,656-byte immutable binary is native-integrated110-1/bin/ktlsp.exe,
SHA2567ac393360fa6784a9bd8a9d2f5e65ca31714cd87f3b02a7244c51e8d451b9d5e.
It uses the temporary explicit System allocator baseline; allocator trial
manifest/lock/main edits are separate and have no demonstrated speed benefit.
Original114 on110 finishes107 passing checks/seven failures. The worker runtime
entry is now captured correctly. The first private generation executes all nine
tasks successfully, then rejects an input without a current owner. Its precise
path was lost when temporary audit files were cleaned up; existing errors now
include task/path for the next targeted reproduction, without changing the
ownership predicate. A later task-not-found error overlaps disabled phase2 and
does not establish the cause of the first rejection. Original114 remains RED.

Gradle's original three first responses all match on110 after a successful
offline194-module import. Root verifies eight artifact references. This actual
run closes naturally with zero owned residuals; it takes613.25s under concurrent
load, including229.328s for the Gradle dump and288.821s for dependency indexing.
Index destruction itself takes43.528s with about31 CPU seconds for125,576
library files/2,067,696 symbols. These are baseline observations, not speed
acceptance. Evidence: recovery89/allocator110-import1/terminal-summary.json
SHA256e7bbe7245b70b1504fcc393e07b895f2a4742d42a73d4d49b865b6eb3510e35d.

The frozen109 automatic-watch reload repeat passes all90 checks/61 first
requests, zero retries/readiness failures and nine real registrations. Root
checks15 artifact references and compares the check-name multisets against107:
all original86 remain, with exactly four native Java checks added. Current-wire
positive error1 and fixed push/pull0 both pass; clean shutdown/owned0 hold.
Evidence: recovery89/gradle-reload109-auto1/summary.json SHA256
c4256937d174d3af9e8dfde3536fbe7799d58f22ab913903069f351c575bcb66.

CEM's original18 cases also pass on110:10 canonical first responses plus eight
Java assertions. The ResourceKey getter has exactly the correct HolderGetter
source target, rather than its TagKey overload. Both sessions exit cleanly;
the224-file CEM footprint, source guards and build-output inventories remain
unchanged. Root checks15 artifact references. Missing-source coverage remains
three with one failed input; no whole-project parity is inferred. The measured
launcher-to-first-tested-useful responses are142.553s/107.551s under concurrent
load, not earliest availability or display latency. Evidence:
recovery89/cem110-original18-1/original18-acceptance-v2.json SHA256
042cd0ae2123f23d753c8b7de04a23e0e53d148af63358eb3b215bd282b8b0d2.

The next speed slice caches the embedded init script under a stable content
identity, avoiding a new Gradle Kotlin DSL class name for every query. Its
concurrent publication/content-version/corruption guard passes, as do64 related
classpath tests. Release111 and a targeted processor-owner reproduction are in
progress. Compilation reuse and performance benefit remain unmeasured.

### Previous109: integrated processors and diagnostic reconciliation

The shared diagnostic merge removes only a fast unresolved-reference error at
the exact range of a current accepted compiler error; other codes, severities
and ranges remain visible. The focused preservation guard passes. Processor
execution now retains current Java overlays in the private selected-source
request and watches bounded original producer inputs before copying them.
The current release includes KSP/KAPT execution and generated-API provenance
checks; general compiler-plugin compatibility remains unproved.

Final workspace passes1,706/zero failed/eight ignored across40 result blocks;
release succeeds. Root verifies all348 current inputs, identical before/after
snapshots, six referenced artifacts and zero live/terminated owned residuals.
Evidence: native-integrated109-1/receipt.json SHA256
4306ed1713d9a4156262e4b7a617b84670dd31fd5531905e77bc90e9ff4b61d6.
The20,833,280-byte immutable release is native-integrated109-1/bin/ktlsp.exe,
SHA2563b7844e8a9a7dea24794442e644db71a7f7aa726e0d08594c06ddf08f4ca7b4e.
The same seven first requests that reproduced duplicate diagnostics on108 now
pass all17 native checks without retry: the Unicode missing-method range has
one compiler error instead of two errors, and the unsaved fix clears both
current-version push and the first standard pull. Root verifies seven artifact
references, including durable source bytes; shutdown is natural, guards remain
stable, and no owned process remains. Empty results still do not claim general
Java API authority. Evidence:
recovery89/java-native-body109-1/owned-1/summary.json SHA256
3b7086cca66202aba08707e481335f7ca88a532e7b97e71a11760603da4caaae.
Actual original114 processor run finishes111 passing checks and three failures,
all obsolete generated Java API diagnostics in phase1. All60 original requests
and four phases finish, with zero owned residuals. Root verifies the binary/raw
references and retains the failures. Evidence:
recovery89/corpus90/native-processor114-editor109-1/receipt.json SHA256
eaaa4cf878f0c6f6cfec99faf678bde05dea0d918976fa141f1ece756c0dac42.
The actual model exports KAPT java_home as null; the processor plan incorrectly
requires equality with the primary JDK17. The next fix captures the stock KAPT
worker's selected JVM rather than treating null as proof. Processor
method certificates currently admit ASCII identifiers only; Japanese/non-BMP
comments have exact ranges, but Japanese generated identifiers remain a gap.

The first portable109 reload run reaches39 passing checks, then cannot start
canonical compilation: its unset AUTOWATCH selects explicit notification mode,
which never supplies a dynamic project-watch ACK. In contrast, original107
used automatic Neovim watches and nine registrations. This is a run-setup
regression, not evidence of a production generation race. Retain the original
run and repeat with genuine automatic registration; do not extend the timeout
or bypass the freshness guard. Static clients still lack this canonical path:
a safe fallback must watch model configuration and closed-source membership
before reading overlays, and retain those watches through publication.

The broad real-Neovim Java scenario on frozen109 passes367 checks with zero
failures/unmeasured and157 requests. It verifies unsaved Kotlin member changes
in Java completion (including removal of the old member), Java/Kotlin array
completion and exact goto, and actual application of override/static renames
followed by Kotlin goto. Root independently checks21 artifacts and10 current
runtime guards, plus representative assertion outcomes. Natural shutdown has
a null shutdown response, exit/code0/signal0 and no forced termination or owned
residuals. Evidence: recovery89/editor109-java1/receipt.json SHA256
b388fbb518b59a7559c1fc4921552ebc728429310ed65728a8a80f9a266171fb.

A separate frozen108 features run reports78 passing checks and two failures:
missing-type diagnostics and definitely-absent explanation in a fixture without
compiler-model completeness. These failures remain visible; do not weaken the
semantic proof requirements to turn them green. Evidence:
recovery89/editor109-features1/receipt.json. This Kotlin-oriented probe checks
returned rename edits without applying them, and explicitly force-stops the
client; it does not establish rename application or natural shutdown.

### Previous108: source and native validation

Javac absolute UTF-16 positions now map to exact UTF-8 boundaries before source
slicing; Japanese and non-BMP comments no longer disable Java import diagnostics.
The focused reproduction compiles and fails on the previous ASCII veto, then
both new tests and two existing related tests pass. The same adapter is used by
the new bounded Java body-diagnostic slice: same-task attributed non-generic
java.base receivers, zero-argument calls, complete current K2 output and exact
diagnostic/AST/source binding. It supports Unicode method names. The Java
observer skips scanning when no relevant error exists, and avoids origin/source
work for calls that do not match an actual error. K1, generated receiver APIs
and general Java body diagnostics remain outside this slice.

The body reproduction compiles before the fix and yields zero instead of two
positives. The initial green attempt exposes a synthetic fixture incorrectly
using UTF-8 length for a Unicode method's raw Javac range; correcting that
fixture to UTF-16 retains both expected positives. The three focused checks and
all46 existing Java-boundary checks pass. Workspace108 passes1,687/zero failed/
seven ignored; release succeeds. Root independently verifies344 unchanged
inputs, both raw captures,40 result blocks and the immutable binary. All owned
Jobs close with zero live or terminated residuals. Evidence:
native-integrated108-1/receipt.json SHA256
3ac354319dfb4ce71d1c395ab8a8dfe9018fe82b02af3a378401c28752baa872.
The20,223,488-byte release is native-integrated108-1/bin/ktlsp.exe, SHA256
1a99c7e073ec9fff117304773f04958386a6c63ea9e2823aac8f8949ab99de37.
Real-Neovim108 completes all seven first requests without retry:16 checks pass
and one fails. The exact non-ASCII method range(6:58-87), native positive to
accepted store/pull and fixed current-version empty push/standard pull all work.
The failed expectation requires one error; the editor receives both the correct
Javac error and a fast unresolved-reference error at the same range. Keep that
expectation: shared diagnostic reconciliation is being corrected. The server
shuts down cleanly with zero owned residuals. Only a mutable build-summary JSON
guard changed; executable, hook, runtime and fixture inputs remain unchanged.
That metadata failure remains recorded separately from the semantic duplicate.
Evidence: recovery89/java-native-body108-candidate1/owned-1/summary.json SHA256
ac68f57cf39af57fe6f85426b07094335ca31a183b56e6cdb783dc5d4c7ae1a7.
Additional existing Workspace trace markers distinguish final-owner release,
Index destruction and subsequent field destruction. One108 repeat reaches
Workspace destruction0.520ms after serve closure, spends11.506s destroying
Index, then146ms on subsequent fields. Index accounts for98.7% of the measured
Workspace destruction interval, with about11.22-11.52 CPU seconds. All three
original first responses match and exit is natural, without forced termination.
This identifies active Index teardown; concurrent conditions preclude a speed
comparison, and shared Salsa facts may still be released after the middle marker.
Evidence: recovery89/shutdown108-natural1/cpu-rss-summary.json.

The AP109 implementation is now in the current checkout. Its overlay regression
fails on missing open-Java membership, then passes after preserving current
overlays in the private stock-selected request. The processor suite passes19
checks with one ignored; the selected-SDK17 native observer control also passes.
A pre-reconciliation workspace run passes1,705/zero failed/eight ignored with
348 stable inputs. Release was deliberately not launched so the diagnostic
merge fix can be included. Final integrated Rust/release and original114 real
editor checks remain pending; neither generated API correctness nor general
compiler-plugin parity is established by these Rust results.

The restored fixed IDEA reference independently passes the original OkHttp
incomplete-code cases: five cases, six first responses,13 semantic checks, one
attempt each. Root verifies57 current guards and four artifact references.
Import reports success. Graceful shutdown still exceeds15s; the owned Job
terminates one remaining process and closes with zero live processes. This is
semantic reference evidence, not lifecycle or speed acceptance. Evidence:
recovery89/corpus90/native-five-okhttp-idea-restored-reference-1/receipt.json
SHA256da2611fcde20049ff78f0e35e997f512564478de1cd23a2ed1ed23e5dee3b122.
The same restored reference passes Spring's original six first responses and
13 semantic checks, including the previously unconfirmed dangling-dot case.
Root independently verifies57 guards and four references. Each run's actual
Gradle daemon log records JDK17. Spring retains the same shutdown wait failure
and exact owned cleanup of one process, with zero remaining live processes.
Evidence: recovery89/corpus90/native-five-sp-idea-ref-1/receipt.json SHA256
6a081b03e1f798b16499a1c7db7dca42aa01a5476927dc7c70411ce1fc80c7e3.

### Previous107 source and native validation

New unsaved Java/Kotlin sources now reuse the stock task's future-source policy
for auxiliary Java compilation. Exact declared inputs remain mandatory; only
current owned editor inputs with an unambiguous matching policy may extend them.
Unknown filters, generated/excluded roots and ambiguous ownership remain
unavailable. The environment identity includes the admitted editor inputs.
The focused reproduction compiles before the fix (one pass/one failure), then
both cases pass. Workspace107 passes1,683/zero failed/seven ignored; release
succeeds. Root independently verified the40 test-result blocks, artifact hashes
and344 inputs before/after the build. The only later changed guarded input is
the harness shell's shutdown-observation help line; compiled Rust is unchanged.
Evidence: native-integrated107-1/receipt.json SHA256
669ce7195ff60881a516a7e8a38d2216951a6a00a1b04b1414f7ec9026f67fa6.
The20,224,000-byte immutable release is native-integrated107-1/bin/ktlsp.exe,
SHA2568e9513e1c453c5732095ea1862939d47228f83d63bcf7b2ee19d488cf6901101.

Reload107 retains all86 checks and58 first requests without retry, through four
accepted generations and clean shutdown. Root verified230 current runtime guards
and four artifact references. Evidence: recovery89/gradle-reload107-1/receipt.json
SHA256f1bfc3ef0c43a7c42f238c39fd4ba7e8e8fff6c30b028c3412df3418a25dddf9.
Its deliberately incomplete Kotlin buffer prevents normal K2 compilation, so
Java compiler diagnostic certification remains unproved by this reload case.
A separate valid-Kotlin/new-Java control reaches normal K2 output and Javac:
one missing-method error becomes zero after the unsaved fix. Three useful
completion/navigation controls pass on their first responses. However, the
existing Java diagnostic selector admits explicit-import errors only; the body
error is not published as a canonical compiler diagnostic. The canonical
observation deadline therefore leaves the native-to-editor certificate
unproved. This is a diagnostic coverage gap, not evidence of a store bug.
The explicit-import control and safe body-diagnostic extension are in progress.

The separate explicit-import control now proves current new-Java native error
to canonical store and standard diagnostic pull. Removing the import produces
native Javac success and a version5 empty diagnostic push. The fixed standard
pull remains unrun: the control incorrectly waited for a clean Java canonical
event, which the positive-only contract does not generate. This observation
deadline is retained; it is not a publisher failure or Java clean authority.
Evidence: recovery89/java-native-import107-control1/owned-1/summary.json SHA256
ca5eaf9a6b5ebcd8dc85ae4256c74de3d487e24457ae2e41cffb330fb78d906a.
The subsequent wire-v2 record distinguishes cleanup-removed private input paths
from durable source evidence. Root verifies17 artifact references and13 current
guards; the recorded wire bytes match native before/after hashes. Evidence:
recovery89/java-native-import107-control1/owned-1/summary-wire-v2.json SHA256
1d9e85489731acf0be58ca83d1e45a1bec165f0f614838c1f2533599c1905c2e.

One diagnostic Gradle ordinary-session repeat keeps all three first responses
and exits naturally in18.172s with zero termination calls. The optional corpus
shutdown observation setting preserves the15s default and records an overrun
as information; the original105 failures remain unchanged. Query-only sampling
shows15.719 CPU seconds across18.222s from just after serve closure to native
exit detection, while working set falls from3,704.82MiB. The final Workspace
field marker occurs18.123s after serve closure and9.365ms before exact native
exit. This supports active teardown work; the last Arc owner and expensive
field are not yet identified. The run indexes1,654,554 library symbols versus
the original105 ordinary run's2,067,696, so it is not a speed comparison.
Root verified six artifact references and15 current guards, with owned live
and terminated counts both zero. Evidence:
recovery89/shutdown107-natural1/receipt.json SHA256
11eb8235bba243c04b29852c08baac340e35a404ecc2ef19d36026e6ffe3c603.

The full101 workspace run compiled and returned1,672 passed/three failed/seven
ignored. Two failures exposed Java lookup of ordinary Kotlin properties versus
`@JvmField`; the third expected Unknown for a compiler-valid projected named
call. Independent Kotlin compiler controls establish Invocation? for that call;
the corrected positive and invariant-slot negative pass in focus102 (four/zero).
The shared scoped-type-parameter/projection correction also passes the real
implicit Java generic delegate body, with exact identity/nullability negatives.

Typed authored Kotlin property facts are now retained with existing source
metadata and forwarded through scoped/recovery queries. Focus103 returns31
passed/one failed across32 cases, including both former namespace failures.
The new failure detected initializer locals incorrectly receiving
property-lowering facts (four Found facts instead of two). The production fix
retains that expectation and also rejects a compiler-proven custom annotation
whose spelling shadows the kotlin.jvm.JvmField qualifier. The selected14-case
property/getter/recovery check now passes with344 stable inputs. Temporary
delegate/enum diagnostics have been removed. Original red evidence:
focus103-property-proofs1/receipt.json SHA256
10662ace5fb22df6bb14b9c579ac45b968337985487249270cce7c502d9e283c.
Green evidence: native-integrated104-1/receipt.json SHA256
cde28870f049da9c5aeb053dc7ee70e9af04252fb406a38f2a955fe30319d5ac.

The first full104 run exposed an unfinished Java binary-generic completion
regression: the recovered declaration had been rejected despite the existing
host-validated header proof. RecoveredTypeIndex now reuses that proof for its
replacement file while retaining foreign Kotlin identity checks. The existing
failing chain and foreign-proof regression both pass. The next full workspace
run passes1,681/zero failed/seven ignored (100.062s including build), with344
stable inputs. Evidence: native-integrated104-1/workspace2-receipt.json and
workspace2.raw SHA256 a40228f0814c8c48a56f8c14a6ef942e09d7680e97b64d2588d788a77488aa5f.
Owned cleanup reports zero live processes; the one terminated residual is
identified separately as MSVC vctip telemetry, not a language-server process.
The fresh release succeeds in64.672s, with344 current inputs independently
verified unchanged by Root. Its20,133,888-byte immutable binary is at
native-integrated104-1/bin/ktlsp.exe, SHA256
91302b4bf64ff95bd7fb030b1193e37bc327da19bab23095266822ffcd3a413a.
Evidence: native-integrated104-1/continuation2-receipt.json SHA256
17cc6e3d3349264434a495800cb3110beaa3752bd4af0f727e2cfc331e8767ba.
The unchanged real-Neovim semantic harness now passes173/zero failed on that
binary. All seven getter first responses match (one attempt each, zero
unmeasured);36 getter checks and the original136 semantic checks pass. Property
completion, exact getter navigation and chain completion update through unsaved
Java changes and restoration. Natural shutdown separately succeeds in this run;
the earlier intermittent shutdown issue is not declared fixed. Root verified29
current runtime guards and12 artifact references. Evidence:
recovery89/getter-native104/receipt.json SHA256
f0e25155371386d01920b92c9edf5a1c92e83c1d9fcce0486e8f7b8ad89acc12.
CEM's unchanged original18 assertions now all match first responses on ordinary
caches:10 canonical requests and8 Java assertions. Exact ResourceKey-only goto
is fixed. Root verified the acceptance references and owned receipts; source,
output and build timestamps remain unchanged. The earlier cache-junction trial
is retained separately (two Java assertions unmeasured after compiler-watch
rejection); it is excluded from the successful denominator. Evidence:
recovery89/cem104-original18-1/original18-acceptance.json SHA256
b34492867e13dfb323e03b3fd926554178f0e6af47933a145bfaa7089481bf95.
Dependency source coverage remains one failed/three missing; typetools is kept
as binary fallback, with no corresponding CEM consumer failure established.
First gated useful completion is97.953s after the first trace event; earliest
availability was not measured, so this is no cold-start/p95 acceptance claim.

The separate external consumer compiles against the current public API:4 tests
pass, release succeeds, and the original three cold first responses all match
with useful completion, exact target navigation and clean diagnostics. Normal
Gradle import and natural shutdown succeed. Root verified11 runtime artifact
references;329 source inputs match the main build. Runtime evidence:
recovery89/external-api104-cold/receipt.json SHA256
58404ca5850ef9df4d71fdd30f8a4ae818f3d6d6214b234e24dd81561ac0d21b.
This proves the existing test provider's separate-project composition, not a
production Mixin extension or general compiler-plugin parity.

The fresh Gradle9 attempt exposes a startup bug before any semantic request:
unchanged build-file notifications repeatedly cancel the model import. File
bytes match pinned HEAD and mtimes predate the run. All nine requests remain
unmeasured; the owned run was intentionally stopped. The shared watch-refresh
fix seeds configuration content identities before watch registration/import and
ignores only known byte-identical Changed/save notifications. Real edits,
creation/deletion and unknown/read-error cases still invalidate. The extended
existing scheduler test passes (one test,3.90s). Workspace105 passes1,681/zero
failed/seven ignored (94.594s including build), and release105 succeeds53.063s
with344 stable inputs. Its20,190,720-byte binary is
native-integrated105-1/bin/ktlsp.exe, SHA256
260238804c6df8b40ea8c88db4c03fdd538b80d38b9bd342f4382998cb29333e.
Build evidence: native-integrated105-1/receipt.json SHA256
20112a894c5a3a29c2b1d710a090b7c31a7d062858dad45b36938a80c4231405.
The unchanged original Gradle9 cases now all have qualified first
responses on105:9 matched, zero mismatched/unmeasured/unqualified, one attempt
each. Both ordinary and incomplete-code sessions converge in generation1
(194 modules, initial model load83.066s), without refresh cancellation. Captured
notifications cover lockfiles rather than unchanged build configurations, so
this runtime alone does not prove the deduplication branch was exercised.
The ordinary session still fails graceful shutdown after15s and is terminated
by the client; its three semantic successes are retained separately from that
lifecycle failure. The incomplete session exits cleanly. Both owned Jobs close
with zero live/residual processes. Root verified10 semantic artifact references,
the direct receipt's four references and11 current input guards. Evidence:
recovery89/corpus90/native-five-gradle-editor105-1/semantic-receipt.json SHA256
f1dab7a856bc277065e3ef5c54efbf15e88277ef16c6a9cdf90c403bcc265b15.
Direct receipt SHA256
d71f1c07b58e269f053d3ca5c37647260fc8b3d97da225c1d611231ff9006a38.
Evidence of the original startup cancellation failure:
recovery89/corpus90/native-five-gradle-editor104-2/startup-watch-classification.json.
Focused receipt: recovery89/watch-refresh105-focus2/receipt.json SHA256
b71b952c1c9c936fc5644b22fa0063a976c53c0b94ad086ada0328945725f23e.
The same105 binary also passes the unchanged Moshi9, OkHttp10 and Retrofit12
qualified first responses. Spring's ten cases also pass:50/50 across all five
projects, with one received response per request and no unmeasured/unqualified
case. Root independently checked those50 attempts and the aggregate SHA.
Root verified each direct receipt's artifact references/current guards/closure.
Moshi and OkHttp exit cleanly; both Retrofit and both Spring sessions retain
the15s graceful shutdown failure. With Gradle ordinary, that is five lifecycle
failures in ten sessions; all owned Jobs close with zero residual processes.
The semantic gate passes while whole-run acceptance remains false. Evidence:
recovery89/corpus90/original50-current105-summary.json SHA256
0f3dbba96ba74b2edb3eb144afd98b0766f09a0c4a94dfb7f8eb3cd66f7b1f49.
Spring direct receipt SHA256
cbb90cff0597c7971c83cca3f6490c08915ad2b87ac2f7a2e5a8ed16c7bc6911.
Receipts under
recovery89/corpus90/native-five-{moshi,okhttp,retrofit}-editor105-1/receipt.json
have SHA256 respectively:
4a2d3d8939f21d4e03f2577dae7d9117bdfc6f9c994decf73e90ff25c0d86fcc,
a8eeb05005f7ae003615166b03058b06c6ffbe5362a4fb7ed053f3a4c61dba3c,
6aabfd31d993cf303003c551e382bb1b1f83534f955e1677a7e5a5930094f511.

Actual Moshi/Retrofit notifications expose a narrower remaining baseline gap:
literal Git pathspecs omit nested gradle.properties/gradle-wrapper.properties,
although their bytes match pinned HEAD.106 changes only those two patterns;
the existing scheduler regression now covers root/nested properties and a Git
subroot, retaining real edits, lifecycle and unknown-input negatives. The focused
case passes, then workspace106 passes1,681/zero failed/seven ignored and release
succeeds. Root independently verified344 current inputs, both raw logs, the
per-target test totals and immutable binary. Binary:
native-integrated106-1/bin/ktlsp.exe,20,190,720 bytes, SHA256
160f57005434bc89da027f63095a10de8e8ba132d9c05cbf284ec5449f847210.
Receipt: native-integrated106-1/receipt.json SHA256
ab7e3666b18634b0b2868e238758e8854be89719167416b52b291e7bce32d6d2.
Moshi106 repeats the same nine qualified first responses with clean shutdowns
and stable guards. Both sessions use generation1 and cached models; no actual
property/JAR notifications occur, so this run does not exercise suppression.
Evidence: recovery89/corpus90/native-five-moshi-editor106-1/semantic-receipt.json
SHA25625f218bfe8cb1a45d60cb14c48c7ed48da5857198d4a4550f9f2202f38ffd404.
Wrapper-JAR notifications are separate from106. The107 source change
hashes only canonical wrappers with adjacent launcher/build configuration,
retains changed/unknown events and excludes resource/library decoys. Its focused
scheduler case and combined workspace/release107 checks pass. Reload107 passes
as described above; no runtime claim is made that the wrapper deduplication
branch was exercised by that probe.

The unchanged dependency/generated-source reload probe on105 records82/86
checks, four failed assertions and two readiness deadlines. All four offline
builds pass; new-library/generated-source completion and initial source goto
work in Java and Kotlin. The v2 source-goto requests occur before source
enrichment is accepted and return a correct v2 binary stub; two later failures
derive from those failed prerequisites. This does not establish stale v1 facts
or wrong source selection after readiness. Root verified230 current runtime
guards and five artifact references. Evidence:
recovery89/gradle-reload105-1/receipt.json SHA256
49a1f446d4963b85ab16c0760814f5dc794e8f3f73e0923023d2a689e151f42e.
Java diagnostics use the fast path: the compiler rejects the new unsaved source
membership, so empty Java diagnostics are not compiler-certified. The106 repeat
retains all86 expectations and raises only the existing readiness wait. It
passes86/86 checks and58 received requests without retry; all feature requests
follow their accepted generation. Exact v2 source goto and subsequent member
removal work in both languages. Root independently verifies230 current guards
and five artifact references. It exits cleanly with zero owned residuals.
Receipt: recovery89/gradle-reload106-1/receipt.json SHA256
d716faa596287d93dab670359c54e62615803956e4442d4434b3a77d0f2d4532.
Summary SHA2562aa316ffbd23fd524889081e29b7bdfca3830dbf2fc005cde19a08f94da2586a.
Per-phase refresh-to-accept N1 values are70.073/51.217/47.865/71.812s. These are
correctness observations under an extended wait, not speed acceptance. Java
compiler certification remains unavailable for the new unsaved buffers in this
probe;107's stock-policy correction passes focused RED/GREEN and workspace checks.

Readiness investigation confirms corpus timing waits for all libraries, while
product completion has no equivalent global barrier. The Gradle105 canonical
completion itself takes7.075ms after indexing; this does not establish earliest
usefulness. Existing FIRST_USE can measure that separately and warms caches;
no new timing framework is required. See recovery89/readiness106/report.md.
Reload/compiler-plugin cases, complete five-project validation and full speed
acceptance remain unproved. The AP-only candidate is not adopted: independent
review found missing original-copy input invalidation and a root-producer
read-set contradiction (processor-ap106-rebase1/review-blockers.json).

The requested test-efficiency audit found build work dominates the inspected
five runs (about344.5s build versus22.4s tests). The inner loop now selects the
changed semantic layer, then runs one workspace gate and one release build.
The109 follow-up finds71.33s of test-profile build work versus0.03s executing
four focused invocations. Two additional attempts to qualify a caller fixture
cost25.703s without reproducing the native failure. Stop that fixture loop and
use the retained actual failure before adding one focused regression. Batch
pending shared-source merges before the workspace gate; keep one final broad
gate and current-release editor check. The two109 workspace runs have different
source inputs, and the release hold launches no native process; neither is
counted as an identical-input duplicate. See recovery89/test-efficiency109.txt.
Existing owned runner APIs/configs are reused without new controller clones.
CPU parallelism is16 after the user's resource authorization; cache/profile
flags remain unchanged and there is no claim of a controlled speedup. Storage
has about435GiB free; the full validation stage added about0.3GiB. Audit:
recovery89/test-efficiency-audit104/audit.md. Fresh CEM18/Gradle9/editor results
take priority over additional synthetic variants.

The full current exporter now completes a normal offline import of the same
pinned CEM project:81.328s, one module/two source sets/four compiler tasks. All
four tasks have complete non-null SDK producer claims and source policies,
where baseline96's main Java/Kotlin policies were null. This proves export,
not future-buffer editor behavior or a speed improvement. Evidence:
recovery89/corpus90/cem-model101-1/receipt.json SHA256
324c1cabbe39b8329ebc5ba389ca8979b08032102ed553dc347a79cd977bf7ef.
The Gradle7.4 full-script Kotlin API incompatibility is corrected independently;
actual7.4.2/9.6.1 controls preserve restrictive/opaque/Unicode-pattern rejection.

The existing Neovim semantic harness confirms all seven new getter requests
return empty first responses on the immutable96 release, with correct cursors,
wire contents and unsaved-edit restoration; other136 semantic checks pass.
Shutdown fails separately (no natural exit before the existing15s observation).
New-release comparison is pending. Evidence:
recovery89/extension102-getter-native96/receipt.json SHA256
5d7a0036dd02b5888f55689f62f4da395fb5fa3e33da6ff55c5923cf128c76f4.

## Historical96 baseline

The96 source fixes caller-specific SDK views, SDK11 legacy signatures, nested
Map.Entry generic result chains, Gradle future-file selection and the exporter
deprecation that blocked real Gradle imports. Shared extension-source readiness
now covers semantic consumers and rejects stale close/reopen requests.
The same29 focused cases pass, including all12 reproduced failures. The full
workspace passes1,650/zero failed/five ignored across40 groups (495.797s including
build); Root independently verified raw bytes/hash, group totals, exit0 and
owned closure. Evidence: native-integrated96-1/workspace-receipt.json SHA256
b7b985b400a76b10468aa7b3e44861803604547a9a3b904d427a577204ffb2e1.
The main release exits0 (544.766s), with clean owned closure and all333 inputs
stable. Root independently verified333 current guards and11 artifact references
in native-integrated96-1/main-ready.json, SHA256
39151e2a0212ac56cdbad2ad1b196c241a8d0c7bbeda5ee92d50cd53cfdc7af0.
Its20012032-byte binary has SHA256
68e6dbc3a1ef9bff6f3e7d87b2bf073d3d68371c2c505640d70686baeb2819f8.
Code is pushed as4f46e449c34af651b355fbdd6eed22095cc86fa5.
External-consumer tests pass4/zero failed and its release exits0. The final
pipeline has stable333 inputs and clean owned closure at every stage. Root
verified the final receipt and external-copy references before subsequent test
edits: native-integrated96-1/receipt.json SHA256
b844b2fc28e9a7520898e6c247da8e0ea64944d142cef0b1dae835641237de8a;
external-copy-receipt.json SHA256
ceac5b7ead775459cd0675ab3079e448f612c212d94adba3b588b01643fa9486.
The copied external binary has SHA256
31c9e34e59b9882179090c1b0d4323923c362c2e5b2f84bc51b2d64c44efa012.
Its original three strict cold first-response runtime checks now pass: useful
exact completion, exact navigation and clean diagnostics, one attempt each.
Shutdown exits naturally0 without termination, all413 runtime inputs stable.
Root independently verified the result bytes and receipt references:
external-library96-cold1/receipt.json SHA256
c4ffb4a12e8795434f3ba262f5c809d14e442a60fcda0069f2bf841187d7e955.
CEM's normal offline import now succeeds (one module,65.600s). The unchanged
original18 finish17 matched/one mismatch/zero unmeasured/unanswered, with natural
owned closure and unchanged private properties. Root verified both owner and
acceptance references. Evidence: recovery89/cem96-original18-96-1/original18-acceptance.json
SHA25611266f8d745782fb59d52e521bc5ba5295730c4982641e563d6733525fbfe851.
The remaining mismatch is precise overload goto: getOrThrow(ResourceKey<T>)
returns its correct location plus the unrelated TagKey<T> overload. A shared
selection fix is outside-checkout work. Both Java/Kotlin tasks also capture null
sdkSourcePolicy/sdkProducerClaims, so CEM future-file SDK ownership is unproved.
A real configuration-only control identifies the cause: Gradle9.6.1's visitor
uses Visit rather than Spec. The two-line outside-checkout correction passes
six actual CEM source-tree controls, retaining restrictive-pattern, opaque-spec
and generated-tree exclusions. This is exporter evidence, not editor acceptance:
recovery89/sdk-visitor96-fixed-control1/receipt.json SHA256
a47b991ea2104dace218c8fb4fb8f1b2e06762a8668d480c437ee81cec75f31e.
Read-only bytecode checks and15 actual configured controls also pass on the
corpus's Gradle8.7,8.10.2 and8.14.2. Combined21 configured controls do not prove
future-buffer editor behavior. Evidence:
recovery89/sdk-visitor96-gradle8-controls1/receipt.json SHA256
b188753cba192ebbc054da62b10cc33fa83735a0c7e3e0b5fcf2d01de6cbb75e.
Original Gradle nine-request
acceptance also remains unproved; the older95 failures below are retained.
AP current-generation support and Moshi dirty
mixed-producer support remain outside-checkout work. No all-six-project,
compiler-plugin, IDEA-parity or speed acceptance is claimed.

## In-progress97: enum completion, source capture and startup work

Two verified slices are committed and pushed: aafbdde790d48bd790795b2e6b9aa4586ac74e66
(source-query overlap) and d4c250ccda690ef17eacc9c2d7a3835375133982 (selected
Kotlin SDK API policy, source capture and exporter runtime reuse). They still
need fresh release/editor validation; no new whole-workspace green is claimed.

Fixtures now use actual isolated project scans and absolute unsaved-source
publication. The earlier completeness helper cleared project metadata and was
not a valid caller-proof setup. Qualified18 first returns14 passed/four failed:
qualified Java enum completion, Java Holder result chaining, an incomplete-world
negative, and an extra implicit-extension body inference assertion. Evidence:
focus97-qualified1/receipt.json SHA256
2c5d262308972ef21967091eaf67cde68179e1516fdf919048598af9a630162e.

Two production corrections now pass their reproduced positives. Java resolves
qualified nested enum namespaces while retaining local/inherited/static-import
shadow checks. Java return inference reuses its existing typed overload selection
instead of rerunning selection with signature help's alternative-arity policy.
The latest18-case run returns15 passed/three failed, with339 unchanged build
inputs, successful compilation and clean owned closure. Root independently
verified results and receipt references: focus98-selection1/receipt.json SHA256
b6a68ee1292f849c65916bb85ab6d5bc672bef87feb09b48f20ff70a212ef27a.
The qualified enum positive and exact ResourceKey-only Java goto/result chain
pass; the unknown-argument and unsaved-flip controls also pass.

Two remaining failures have invalid incomplete-world preconditions: the manual
diagnostic scan-complete flag does not withdraw semantic caller proofs. The enum
negative previously passed trivially while its qualified namespace was broken;
its actual value-shadow controls pass now. Both negatives are being corrected
to exercise real pending-generation transitions, with proof assertions. No
production lifecycle defect is established by those two failures.

The corrected19-case baseline returns16 passed/three failed. The private
pending-generation proof-withdrawal/restoration test and the qualified enum
pending negative pass. Holder's new precondition still fails at the general
completion status; its paired-positive debug output precedes the generation
change and proves no post-change argument type. The actual strict argument path
must be checked before classifying a production defect. The other failures are
the independently reproduced duplicate-release case and the getter body below.
Evidence: focus98-pending-duplicate-red1/receipt.json SHA256
95b9ea1383652ef9aee999a50ffdffd0d3e95346150a7b10cf945140ddd8cab2.

Ordinary Kotlin access to a Java getter as a property is now corrected in the
shared value-selection layer. The qualified tests-only baseline returns one
passed/two failed: both explicit/implicit typing and unsaved Java edits produce
Unknown. The boundary negative uses a checked receiver-expression cursor; its
first draft accidentally targeted the import and is not production evidence.
Evidence: focus99-getter-qualified-red1/receipt.json SHA256
0ccca38ffdc2d6f1f168cfe3748426f4a3e413b18713638534cd8caf32af3a9e.
All three getter tests pass after the correction: useful incomplete completion,
precise types, exact original-getter navigation, both-spelling references,
safe refusal of mixed-spelling rename, language/access/shape boundaries and
unsaved return-type updates. Ordinary method lookup retains its original
traversal; getter discovery is limited to value/completion queries. Prefix
completion still gathers all getter properties. Acronym/is getter forms remain
coverage gaps. The existing rename-reference cap is also applied before an
alias scan; that tiny follow-up is not yet rerun.

The latest23-case run returns21 passed/two failed, with339 stable inputs and
clean owned closure. The strict pending-argument negative and real project
refresh proof tests pass. Root independently verifies counts and five receipt
references: focus99-getter-green-shadow-red1/receipt.json SHA256
fe74e6c2a069c1a6b479a25debe9f0bb6d03a676f31f1e6af5bdc482262a0e6b.
Remaining failures are imported-type completion incorrectly winning over an
inherited Java value after an unsaved edit, and the generic extension-body
delegate result. For the latter, bare `level`, `checkNotNull(level)` and
`entity.level` now retain world.Level; the concrete Java generic call also
retains HolderLookup<Enchantment>. The body using projected symbolic `T` remains
Unknown. Consumer argument/receiver, exact ResourceKey-only goto and useful
result chain already pass, so native CEM's overload mismatch is still not
reproduced by this reduction. Temporary ENUMDBG/HOLDERDBG/DELEGATEDBG diagnostics
must be removed before commit/release. Full workspace checks and fresh-release
Neovim checks, including the added semantic getter probe, remain unrun.

The proven source-visitor corrections are adopted. An invocation-scoped compiler
loader cache is also adopted after actual Gradle8.10.2 resource/reuse controls
and a late JAR mutation control: valid model rows print first, then service
closure fails with the exact runtime-change error. The same existing controls
also pass on Gradle7.4.2,7.5 and9.6.1; this does not prove the whole exporter.
The first normal Rust importer control catches a missing runtime-provider argument in
the opt-in observer. All observer callers are corrected; subsequent direct
validation initially stops at a missing Gradle accessor-cache metadata file
before model output. The exact corrupt temporary-cache entry is preserved in
quarantine before one explicitly recorded post-repair pair. Direct late-mutation
rejection and normal cold/warm configuration-cache semantics then pass. One
owned Java process requires cleanup in the cache probe, so its lifecycle is not
a clean pass. The subsequent normal Rust importer control passes1/zero failed,
rejecting the complete model after the genuine late runtime-change failure.
Evidence for the original service controls:
recovery89/compiler-runtime96-controls1/receipt.json SHA256
25126d725f38deebf32d471fa72a276302e270ae6301069a6e23bcaef9584720.
This is a reuse/safety result, not a measured startup improvement.

CEM's trace exposes another serial startup dependency:65.692s model import,
58.393s binary preview, then70.747s source query. A source-query overlap fix is
adopted after its ordering-barrier baseline fails while cancellation,
stale-generation, failure and existing binary-first RPC controls pass. All three
production controls now pass. Potential overlap is not a measured speed gain.

The genuine96 Gradle slice measures all nine original first responses: three
qualified useful matches, five missing AtomicInteger positives and one
unqualified negative (its paired known receiver also fails). Both editor
processes still require forced shutdown after serve.closed. The normal model
contains194 modules/1,176 source sets/1,338 compiler records, all with null SDK
source policy/claims. Besides the adopted exporter correction, actual Kotlin
2.0.21 accepts -Xjdk-release=1.8 while the then-current classifier rejected it. An
independent compiler oracle emits identical class bytes for release8 and1.8.
Both the useful SDK completion/navigation case and the untouched captured
SourceSet first fail release1.8 selection, then pass after normalization and
target/release agreement validation. The actual captured null policy remains
an independent caller-admission failure. Root verifies349 inputs and artifact
references, and independently checks both ignored stages'1/zero results,
exit0 and empty Jobs: focus97-sdk-green1/receipt.json SHA256
e1d9c60669280a88be9615b6be55abe502118589f9f0f5a1de7f064fb0dbcee7.
Equal duplicate release flags are also accepted by the compiler. A separate
tests-only run reproduces the missing API inventory for numeric8+8. The parser
now retains agreeing releases and still rejects conflicting, invalid, future or
target-inconsistent settings. The exact SDK caller regression passes after the
four-line correction, including useful Atomic completion/navigation, unavailable
API/wrong-receiver negatives and the independent null-policy guard. Formatting,
all339 input guards and owned closure pass; Root verifies the receipt and five
references. Evidence: focus98-duplicate-green1/receipt.json SHA256
b5308ecc5818e0271a63844738c0c5c80581c1a9340c0afbdd76fbf4d4f05fff.
Only numeric8+8 has a dedicated duplicate compiler oracle; no fresh whole-suite
or Neovim acceptance is claimed for this slice.
No original Gradle acceptance or all-five-project completion is claimed.

## Previous95 broad and core editor checks pass; Gradle import blocked

The focused95 snapshot passes18 tests with zero failures (2.19s test execution):
caller-specific SDKs, unsaved dotted/Unicode Kotlin filenames, nested SDK
completion/navigation, first extension responses, cancellation, invalidation and
foreground/background queue progress. The filename and Map.Entry cases first
fail useful-completion assertions; queue ordering first fails while its two
controls pass. Evidence: recovery89/focus95-green.raw,
recovery89/sdk95-filename-red.raw, recovery89/sdk95-nested-entry-red.raw and
recovery89/extension95-priority-red.raw.
The final revision retains17 of those tests and strengthens the nested SDK case
to require String completion after Map.Entry<String,String>.getKey(), for both
qualified and imported Entry. A proposed global parameterized-owner rejection
was removed to preserve existing source-nested completion; full outer-argument
fidelity remains a coverage gap. Its full workspace run returns1,634 passed,
three failed and five ignored; all17 retained focused cases, including the
strengthened String chain, pass. The three failures reproduce in isolated
processes. Their fixtures incorrectly demand lazy materialization of an eager
stub, edit the wrong Kotlin producer after the shared fixture gained a second
task, or omit the new explicit caller ownership metadata. Corrections retain
the expected completion items, exact navigation spans and file contents.
Evidence: native-integrated95-1/receipt.json SHA256
2c3b74434c7a47f6916e0c51bf4120f5ab9fea12947f61964488a5a548839e9e;
recovery89/cem90/sdk95-isolated-three1/receipt.json SHA256
202d59f40fd7be5ea0684d31114b1049d00de25f68fb8e9be0a24af630142ffa.
The next focused run passes two cases and reaches a later unready-state
assertion in the third: metadata restoration alone cannot resurrect retired
SDK facts. That test now requires absence before actual SDK batch publication
and the original useful result afterward. The remaining focused case now passes,
and the full95-3 workspace suite passes1,637/zero failed/five ignored across40
groups (290.75s including build). Root verified the raw artifact and independently
summed every group; regular output file closed and owned jobs ended without
forced cleanup. Evidence: native-integrated95-3/workspace-receipt.json SHA256
555d4a97a90ac7b0c209f058efb2ea10e9db9c1f1ccefa5d150f8711b00750d2.
The main release build also exits0 (491.016s), with no live owned jobs or forced
cleanup. Root verified all327 current input guards and12 artifact references in
native-integrated95-3/main-ready.json (SHA256
7483887117ba55c716bdafdd85e545ce184e43278943083498e180fb921f0e4f).
The immutable19987456-byte binary has SHA256
1c3a21e48adfb2fa5349d35cd29d2b2c867cafd777b2ce134e8830f166236a61.
The same copy passes the real Neovim basic9, semantic137 and Java373 checks
(519 total, zero failures). The initial console-line count missed six coalesced
Java checks; the structured result records373 and157 first-attempt responses.
Its bound semantic-acceptance.json SHA256 is
a5fece777634a6e3830ff20d33e6f26c171d80a98b48f9f0b12b890f9dc31cc7.
All three owned runs close without residuals; Root
verified44 artifact references in recovery89/editor95-core1/receipt.json, SHA256
1d05621ff77cd8c4ed9eaf22befe3301c6536ce9783d818365e5f2061d982d9b.
The fresh Gradle import fails both normal offline/online attempts before model
capture: new exporter line510 uses deprecated String.toLowerCase(Locale.ROOT),
which Gradle8.14.2 treats as a script compilation error. The nine SDK requests
remain unrun. The outside-checkout one-line correction passes a minimal
same-Gradle warnings-as-errors causal pair (deprecated exit1, fixed exit0),
with clean owned closure. This proves script compatibility, not a full export;
recovery89/sdk-native95-import-failure/causal-pair96-1/receipt.json SHA256
2abb0a34a636dfb93f38df0a237bac170a179f74d374c82c9724d21a979ce9e9.
External-consumer tests pass4/zero failed and its release build exits0. Root
verified all327 final current input guards and17 artifact references before
the96 source changes. Final receipt native-integrated95-3/receipt.json SHA256
d16c912bf08014ee7abd5053ae9321f2a5e9fab1812bbc3c03c1c758409451c4.
Its20045312-byte external binary has SHA256
6548bbaf985f4f8a4c4713784a3813abe2dfc92cd0c11cd0c82a297ad82f820a.
Both shared libraries recompile in the independent consumer build despite lock
alignment; third-party dependencies are Fresh. This is no build-speed result.
No current all-six-project acceptance or performance claim follows.
The original18 CEM editor cases finish against the immutable95 copy:
five raw matched/nine mismatched/four unmeasured, with both scenarios naturally
closed and private properties unchanged. All nine valid canonical positives are
missing; its only canonical match is the unknown-receiver negative control.
No successful current model load is observed, so these are not CEM acceptance.
Evidence: recovery89/cem95-original18-95-1/original18-acceptance.json SHA256
4dda96d9ad11f88959a3d4ecf554cf67f39ed3826543395e96faab9f5d9b8059.
Their normal Gradle9.6.1 offline/online imports also fail at the same exporter
line510, without a project warnings-as-errors setting. Thus the correction is
required for CEM as well, not merely strict Gradle8 builds. Both failed imports
remain recorded; no model or editor success is inferred from fallback indexing.

The96 tests-only baseline compiles and returns17 passed/12 failed across two
library groups: three SDK11 failures, three task-path-policy failures and six
extension consumer/reopen failures. Root verified334 current input guards and
the raw result before applying production changes. Evidence:
focus96-red1/receipt.json SHA256
490ab9d642dc1910344d696a28917e6262d96fbe83763a9d44a61ce80e8d6555.
The reviewed production patches are now applied and formatting passes. The
same29-case focus passes all29/zero failures across two groups (2+27), retaining
the17 positive controls and turning all12 reproduced failures green. Root
verified its raw hash, exact group totals, exit0 and owned closure. Evidence:
native-integrated96-1/focus96-receipt.json SHA256
c47c8eb83d74adba005ea1e8f74b64e897b56fbc2f40d1efa54c0eb6c5c24fe3.
The full suite now passes as recorded above; release/native validation remains pending.
They correct SDK11 legacy ct.sym paths (one actual194-model producer uses
SDK11/release8), capture stock Ant exclusions for future-file ownership, and
share the first-source wait plus close/reopen input identity across semantic
consumers. The one-line exporter lowercase correction is included. SDK11 layout,
captured Ant defaults and consumer-wait draft manifests are respectively under
recovery89/sdk11-layout95-draft/status.json,
recovery89/sdk-ant95-draft/promotion-qualification.json and
recovery89/extension96-consumer-{tests,production}.patch. Their Rust execution is
pending for the new production revision. The Ant draft's normal exporter capture is verified separately from
the extracted matcher: six task records include the captured exclusions, while
custom-pattern future roots stay unavailable; capture receipt SHA256
b11311b570d210fdb9b98fcf08497d41b34bfae73af5bfe968b8add5f1df66ae.

Earlier94 observations retained below:

Caller-specific SDK views and per-file extension-source producer waits are now
applied to the checkout. The three SDK LSP tests pass after the fix (zero failures,
0.39s test execution); formatting also passes. The integrated runner stopped after
Cargo exited0 because one owned residual required termination. Its identity was
not captured, so lifecycle acceptance remains failed and is not attributed to
the SDK change. No broad suite, release build, or extension check ran in that
pipeline. Evidence: native-integrated94-1/receipt.json SHA256
418e699fe43206479c29b5827e11e0d86917c89cf8c0283886d3bbb808dabfcc.
The runner writes a regular raw file; its EOF flag is derived from owned-process
cleanup, not an independently observed pipe EOF.
Direct execution of the six compiled extension tests then closes all owned jobs
without residuals: three pass and three fail their useful-result checks. The
three failures use an invalid test-provider contract (Direct name linkage with
an Authored rename subject); the core rejects that combination. The fixture now
uses Target, as the real external provider already does. Expected completion,
navigation and diagnostic results are unchanged; a new green run remains needed.
Evidence: recovery89/cem90/focus94-extension-direct1/receipt.json SHA256
473f5d7739a4ec33f7a88a97f7af1f6b2493973438f4e3c09c8b39a0aabc9699.
A separate three-test queue probe reproduces an active source waiting behind
another unrelated batch (one failure, cancellation and background-progress
controls pass). The worker now prioritizes active waiters while reserving one
background slot per batch. This production change still awaits its green run.
Raw baseline: recovery89/extension95-priority-red.raw.
The baseline
SDK tests run3/3 and fail their first useful AtomicInteger control. All six
extension readiness cases also reproduce premature first responses: four have
independent initial results, and two sharing a Windows timestamp-based fixture
directory are qualified only by their separate fresh-process executions. Both
original colliding results are retained. Fixture IDs now add an atomic counter,
without changing expected semantics. Exact upstream Shadow bytes are preserved
with a narrow .gitattributes rule and verified against the fixture provenance.
Evidence: recovery89/cem90/focus94-red1/receipt.json SHA256
86e710039831f112a657209a033ae8f48b58785133ac1db588bf738e827626c4;
recovery89/cem90/focus94-isolated-red1/receipt.json SHA256
3900970fae450c34f053912731866ea32dd3c02765e1c83bac4ae2f8e7763e15.
New shutdown landmarks preserve field destruction order and public Workspace
field moves; they diagnose the retained large-project cleanup failure, not fix it.
The independent consumer lock now pins the same compatible shared dependencies
as the host, with zero shared version/source/feature drift in actual metadata;
host dependencies and profiles are unchanged. Root verified eight corrected
artifact refs before copying the55,108-byte lock (SHA256
8cdf04b6363f87aa4aecd8c62920928eea894e9845e523cc9fa86e487ac1c4c5).
The first metadata receipt incorrectly referenced a mutable pre-resolution seed
and remains rejected; the correction binds separate immutable seed/resolved files
without rerunning the experiment. Evidence:
recovery89/cem90/external-lock94-evaluation/receipt-corrected.json SHA256
e644dea69a0c5ae455bfb30e65a271a3808210cd97b69d53decdb016926b8e4e.
Actual build reuse and the remaining94 production checks are still pending.

## Verified93 checkpoint and actual project results

The narrow SDK21/target17 InProcess compiler profile now passes the40-test
compiler group after its new captured-profile regression first failed. Other
execution strategies and opaque daemon arguments remain rejected. This is not
yet a passing native CEM reload86 run.

Source enrichment now compares unique modules by build/project identity while
preserving every other model fact and all inner ordering. The four source-snapshot
tests and the retained actual194-module pair pass after reproducing the failure
before the fix. Duplicate identities and binary/compiler/build-file drift remain
rejected. Actual normal Gradle194 import on immutable93 now accepts source
enrichment despite156 reordered positions:194 modules/1,176 source sets retain
49,390 source artifacts and72,307 POMs, with identical non-source facts. The
accepted current-generation dependency index publishes102,260 files/1,825,236
symbols. Root verified nine bound artifact refs and the post-guard trace; this
proves the ordering fix through the real import path, not all-project semantics.
Ten dependency-source failures and27 missing sources remain in that index.
Shutdown still requires forced cleanup after the existing15-second bound; the
owner retains exit1/AssertionError rather than converting it into lifecycle
acceptance. No nine-RPC replay was performed while its SDK gap is unchanged.
Evidence: recovery89/corpus90/native-five-gradle-model93-1/normal-import-acceptance.json
SHA2564bb660108b8a91355662308aa79c1437fd8de6c53061c33284e821ae77ba3d36;
original receipt SHA256f1bb2c38f71a5421a2f75dca32f7f31ba0210d2d5a831b0e96722eff70ea61f7.

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
required. External-extension tests also pass4/4 and its release build completes.
Root verified all311 current input hashes, eight final build refs and11 external
copy refs before applying the next test-only94 slice. Final93 receipt:
native-integrated93-1/receipt.json SHA256
9345416cafaf3b5824bb89aabe726cc2a696d790e1033c79c0d21e8ea8e226e5.
The external19,747,840-byte copy has SHA256
3231b5a0d23839f21d85136ce0c7e5565b4da4be9de7d586e22d2c8772aff7eb.
Its unchanged native50+shape40 corpus now passes90/90 single-attempt outcomes,
including all three formerly failing Kotlin Any cases; shutdown is natural0.
Receipt: external-shapes93-native1/receipt.json SHA256
bcf14926ad667d7fbc87897f1f405bb7306d5433c0551195b04015bb69d83b4e.
The old independent consumer lock resolves56 dependency/version pairs differently
from the host. An outside-only host-seeded metadata control now resolves with zero
shared version/source or feature drift, preserving independent consumer builds;
actual artifact reuse remains unverified. The original lock bytes are retained
in the93 copy receipt. No production dependency or profile has changed.
Focused evidence: recovery89/cem90/focus93-summary.json,
SHA256e6ead20f7b26748be641fa5826b868b750d78c7cac53224ef8921e8590a0a439.

The processor114 failures are now bound to an absent current Java diagnostic
store: the active KAPT/KSP bridge was not admitted, rather than a compiler run
certifying the obsolete calls. An independent compiler control produces all
three expected missing-method errors. A new isolated prototype now owns actual
KSP-to-stubs-to-active-KAPT generation for both v1/v2 on the same exact dirty
wire inputs. Existing K1 API headers accept both generations; structured javac
returns exactly three expected errors per generation while paired current APIs
remain useful. No saved output substitutes for generation. Root verified20
bound artifact refs: recovery89/corpus90/processor-current-input-output-proof93.json
SHA256f59c9aa5d073c23c660f386514fa6e02106f397e2dae120e116b13c3213f93f4.
A further real phase-separated generation closes KSP, stubs and active KAPT
successfully, with the latter two reading sealed current predecessor outputs
without rebuilding processor tasks. Five native read watches remain valid;
a separate change-and-restore control invalidates despite identical final bytes.
Root verified nine bound refs: recovery89/corpus90/processor-rust94-draft/
separated-phases2/receipt.json SHA256
997c89799bf5e81fee9761e0dee7301edbe9d6af6309bcdcc77803a9b874823f.
The first broad-build-directory watch failure is retained. This prototype does
not prove reuse across edits. The production Rust adapter and method-diagnostic
admission remain outside-checkout work; original114 remains111/3. Saved generated
files alone must not qualify dirty inputs.

The external95 genuine cold-source check now passes all3 first responses:
completion requires fixture.Target(I)I and the exact `twice` replacement edit,
goto reaches the actual target, and the full diagnostic response is clean.
The unchanged mixed Sponge project indexes15,604 library files from3 dependencies
with zero failures/missing sources. Exit0 is graceful with no termination calls
or owned residuals; all413 runtime guards remain stable. Root verified the raw
result SHA and exact single-attempt outcomes. Evidence:
external-library95-cold1/receipt.json SHA256
163a2ca284680f3e7655e5bf7ec6da66a1a2409831dd9314f7466bb7ec3ec0a9.
Earlier20 real-library checks are reused, not added to this run's denominator.

Historical control: the genuine external-library run using the external91 binary passed20 new source-navigation/descriptor/
diagnostic checks but retained a baseline binding failure. Closed trace comparison
shows that failing completion finished before the selected Shadow.java source
was materialized; the first responses succeed in a separate warm diagnostic run.
The scoped unit regression passes and does not reproduce this readiness race.
That historical run did not prove a stable fix. Exact hydration publication
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
