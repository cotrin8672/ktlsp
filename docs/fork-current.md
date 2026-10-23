# Current fork state

Updated 2026-10-23 (Asia/Tokyo). Goal active; IDEA parity and complete valid-code type coverage remain unproved.
Current user priority: eliminate valid-code Unknown results before further warm latency or memory tuning. Retain uncertainty for invalid or insufficiently proven cases.
History: docs/fork-acceptance.md, docs/fork-corpus.md and Git.
Prospective speed targets and comparison rules: [performance budgets](fork-performance-budgets.md).
BASE denotes C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1.
Other evidence paths below are relative to RUN:
C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda.

## Active Unknown coverage

### Checkpoint120-24-2: three shared analysis gaps fixed

Workspace1803 PASS,0 FAIL,9 ignored across42 targets; release builds successfully.
All576 guarded inputs remain unchanged. Immutable binary SHA256:
`264837343aff483ef30ece639c2f4a5aa14dbca7f1daa241df57a821cfaafe48`.
Both Cargo stages exit0/EOF/owned-live0; only exact owned MSVC vctip helpers need
cleanup. Runtime editor processes close normally without forced termination.

| Same-release editor evidence | Result |
| --- | --- |
| Existing Java/Kotlin surface |427/427;61 first member RPCs |
| Five additional projects |50/50 first responses; no mismatch or unmeasured result |
| Captured outer method bounds |8/8 first RPCs;4 owner/consumer pairs, incomplete completion and sole exact goto |
| Failed-source binary APIs |9/9 original checks; stronger Persister singleton/range audit also passes without additional RPC |
| Actual CEM FQN getValue |8/8 checks;6 first RPCs |
| TypeTools overload |4/4 first responses; sole compiler-selected Class/Class declaration |
| Original CEM scenario |Normal phase has7 semantic matches,0 mismatches,2 unmeasured compiler checks; incomplete phase not run |

Captured outer class type variables now reach method bounds through exact lexical
ancestor scopes. Existing owner-instance substitution proves actual type arguments;
raw/star, static, non-inner and unknown scopes retain uncertainty. Java interface/
annotation member classes additionally enforce their implicit static boundary.
No inference representation was added. Cache v45 refreshes changed parsed facts.
Independent Javac21/Kotlin2.4 controls cover all four source combinations; compiled
Outer.Inner has `<V:TT;>(TV;)TV;`. All61 fixture hashes match the actual compiler
inputs/outputs; unrelated Outer/Shadow class bytes remain unchanged.

Static Java class receivers in Kotlin navigation now reuse type binding only after
ordinary value/lexical/implicit receiver resolution. Unknown-typed shadowing values
remain authoritative; a bare class expression is not globally made a value.

Generic override comparison now checks alpha-renamed declaration binders, bounds
and nullability. Independent concrete argument mismatches and a narrow declaration
specificity proof select Persister.read(Class,String), not its Object overload or
inherited interface candidates. The actual first response contains exactly one
compiler-certified range. Unknown arguments, unsupported bounds and projection
proofs retain uncertainty. A previous Unknown guard now legitimately resolves an
independently Javac/Kotlin-certified call; its original shape and exact navigation
are retained alongside a separate unsupported-competitor guard.

Intermediate failures remain recorded: `persister129-{green1,green2,focus3}` include
compile-only failures or the unresolved two-candidate result; `captured-owner128-green1`
has144/145 generic checks before the independently verified guard upgrade.
`native-integrated120-24` retains1800 PASS/3 FAIL/9 ignored and no release build:
the new rejection guard accidentally blocked an existing unbounded Class<T> nominal
head proof. Its restoration passes all five focused controls and the full24-2 suite.
No old Spring expectation was weakened.

Current CEM gap is explicit: its canonical Java diagnostic receipt was not accepted,
so manual pull count is0 and two diagnostic checks are unmeasured, not semantic
mismatches or a passing timeout result. Read-only investigation sees
`stale-kotlin-source` then `kotlin-project-model-pending`; whether refresh completion
reenqueues the compiler work is under investigation. The original failed run is
preserved and was not retried. CEM's separate completion/navigation checks above pass.

Evidence: `native-integrated120-24-2`, `java-kotlin-msc121-release24`,
`final-corpus120-24/summary50.json`, `captured-owner128-native24/receipt.json`,
`fallback-api128/release24-2-summary.json` and its Persister singleton post-audit,
`cem-getvalue122-native-release24/receipt.json`,
`oracle120/typetools-editor-release24-strict1/strict-audit.json`,
`cem-startup120/editor-original26-release24-serial1`, and
`captured-owner128-final.json`. Nullable override and independent-argument compiler
oracles: `generic-nullability128-oracle1`, `persister129-independent-oracle1`.

Windows long-path compatibility remains separate: the new binary API tests use
fresh guarded short private caches (162/168-character targets). They pass actual
Neovim file/range reads without claiming support for the original271-character
path. The original23 REDs and both baseline cache inventories remain unchanged.
Retrofit/Spring source-text failures remain6/11 respectively; usable binary API
facts do not make those source texts available. No new warm, memory, general
compiler-plugin or IDEA-parity claim is made. Valid-code Unknown gaps remain.

### Previous checkpoint120-23

Checkpoint120-23 passes1801 Rust tests,0 fail,9 ignored across42 targets,
and builds release with576 stable inputs. Immutable binary SHA256:
`c3ed4314c4586b5566ff2f2bc1740dc00999988af0a13f0cd8fdc7651f54f64e`.
Both Cargo stages exit0/EOF/owned-live0; each exact retained MSVC vctip helper
requires owned-job cleanup. Actual LSP runs close cleanly with no forced cleanup.
Goal remains active: new library navigation cases below expose real gaps.

| Same-release editor evidence | Observed |
| --- | --- |
| Java/Kotlin editor baseline |427/427,61 first member RPCs |
| Original CEM normal/incomplete |26/26,11 first RPCs |
| Actual CEM FQN getValue |8/8,6 first RPCs, both languages |
| Strict TypeTools overload |4/4 first requests, sole compiler-selected Class/Class source range |
| Five additional projects |50/50 first requests: Gradle9,Moshi9,OkHttp10,Retrofit12,Spring10 |
| New receiver-owner method bounds |8/8 first RPCs,4 Java/Kotlin owner/consumer pairs; incomplete completion and sole exact goto;16 semantic/delivery assertions |
| New OpenJSSE public API |6/6 functional first RPCs, both languages; cache-copy isolation fails separately (see below) |
| New failed-source binary API probes |4/9 strict RPC matches: all3 completions and Persister class goto pass;5 navigation checks fail |

Original editor expectations remain unchanged. All runtime processes close normally.
Evidence: `native-integrated120-23`, `java-kotlin-msc121-release23`,
`cem-startup120/editor-original26-release23-serial1`,
`cem-getvalue122-native-release23/receipt.json`,
`oracle120/typetools-editor-release23-strict1/strict-audit.json`,
`final-corpus120-23/summary50.json`, and `receiver-bound128-native23/receipt.json`.
New API evidence: `openjsse127-native-release23-attempt3/metadata/receipt.json`,
`fallback-api126/r-release23-1/receipt.json` and
`fallback-api126/s-release23-1/receipt.json`. Two OpenJSSE prelaunch adapter
failures had zero RPCs; the successful functional run is the first semantic run.

### Delivered Unknown fixes on23

Fixed source-encoding authority for a fixed compilerArguments/bootclasspath value;
encoding/profile/merge/raw-argument guards remain. Actual OpenJSSE1.1.12 retains
its explicit UTF-8 policy:202/202 Java sources parse without syntax errors,
5920 symbols;198 declaration-bearing files publish (four package-info files have
no symbols). Actual JAR API is independently Javac21-certified. Source encoding
failure is not a persistent negative cache; current policy is revalidated before
symbol-cache reuse. Focused evidence: `openjsse127-{red1,green1,actual,final.json}`;
15 encoding tests pass. Actual OkHttp failed-source count drops1 to0, missing0,
library files37544 to37742.

Fixed failed-source binary publication: a decoded empty marker interface no
longer discards other useful classes in its selected artifact. Every batch still
requires exact selected origin and Stub provenance plus proven decoded type or
member facts. Opaque headers, partial sources and stale/ambiguous origins remain
rejected; source completeness is not upgraded. Focused LSP worker test passes
(`joint-unknown127-green1`); that attempt retains143/145 generic checks and two
obsolete unsupported expectations as a RED, not a full GREEN.

Receiver-owner method bounds now retain proven `<U:T>` relationships without
new serialized fields; cache v44 invalidates changed bound facts. Nearest-owner
and complete lexical facts reject inaccessible static/companion/foreign scopes;
raw/star missing bindings do not resolve through a global same-named class.
Independent Javac21/Kotlin2.4 controls certify positives and reject wrong arguments.
Actual compiled Box.class is major61 and its `<U:TT;>(TU;)TU;` signature is checked.
All60 fixture hashes match. Final focused147/147 (generic145, Signature1,
compiled-JAR Java/Kotlin completion/goto1): `joint-unknown127-green2`.
Compiler/fixture evidence: `receiver-bound127-{oracle1,legacy-oracle1,fixture1}`.
Actual8 first Neovim requests also pass (`receiver-bound128-native23`).

### Remaining navigation and source coverage

New binary API probes use model-only seeds, fresh symbol/extraction caches and
strict baseline file-set/byte guards, which remain stable. Retrofit Persister.read
completion and class goto pass; method goto returns20 locations from both Persister
and Serializer. Independent Javac21 compilation of the unchanged complete call
`p.read(type,text)` selects `(Class,String)Object`, confirming unresolved navigation
ambiguity. Spring ContextFactory completion passes in Java/Kotlin, but Kotlin
class goto is null. Three other goto responses have valid materialized stubs and
correct ranges: their271-character Windows paths fail both Neovim readfile and
libuv open. The earlier missing-file classification was incorrect; extended-path
reads prove the files existed during the original run. This is a real editor
access problem, separate from semantic resolution or lazy source publication.
The isolated RUN prefix contributes108 characters: the same target under the
normal default cache is163 characters. Preserve the long-path failure as an
editor compatibility limit; a shorter guarded private cache can validate the
unchanged navigation semantics without claiming long-path support.
Keep the original strict REDs and fix semantic causes before new runs.
Audit: `fallback-api126/release23-phase-audit.json` and
`fallback-api126/persister-call-oracle23/receipt.json`.

Source text limitations remain separate from available API facts. Retrofit still
has6 failed-source artifacts and missing-source reporting9 (was4), while published
library files grow67339 to70600 (+3261). Spring retains11 failures and missing19
(was10), while library files grow153792 to157308 (+3516). The extra missing-source
counts arise when previously skipped failed-source artifacts now publish selected
binary facts; source text did not become available. Two stale local Spring artifacts
remain excluded. Exact causes: `final-corpus120-22/other-source-coverage-causes.json`;
phase counts: `final-corpus120-23/ors-pairs-summary.json`.

OpenJSSE functional6/6 is NOT a passing isolation proof: a full symbol-cache copy
keeps its old absolute source URI, and goto adds one SSLParameters.java to the
original private test cache. Its114 existing files remain byte-identical, with no
removal or change; the additional file is preserved. See the fileset delta and
`openjsse127-native-release23-attempt3/final-summary.json`. Cache relocation support
is unestablished, so do not label this a product bug without auditing the contract.
Subsequent new API probes use model-only seeds and pass the strict isolation guard.

Captured outer/parameterized-owner method bounds still return Unknown; do not
count them as supported. No new warm or memory measurement is claimed. Broader
IDEA/compiler-plugin/reload evidence below belongs to its recorded revisions and
has not all been repeated on23. Previous selected22 results remain valid at
SHA5f9868838966253fda207ab91aa8315e06014973ffdbb2d8815d1d375179bc27
and commit c51cde6, already pushed.

### Previous editor-accepted checkpoint120-19

Source through8f65812, checkpoint120-19 passes1786 Rust tests,0 fail,9 ignored
(42 targets), and release with575 stable input guards. Its SHA is
8100bcb57a4ee753c0a25d624425fbc53936d2de31a847d34e5a3851efb8853b.
Both Cargo stages exit0 with closed output and zero owned live processes;
each stage's retained MSVC vctip helper requires owned-job cleanup. Real LSP
runs close normally: Java415/415 (53 first member RPCs), controlled serial
CEM26/26, and five-project50/50 all pass on this same binary. Evidence:
native-integrated120-19, java-classliteral121-release19,
cem-startup120/editor-original26-release19-serial1 and
final-corpus120-19/summary50.json. No new performance claim is made.

The original TypeTools consumers now pass4/4 first requests: Java and Kotlin
completion and unique goto. Each goto resolves only to the compiler-selected
Class/Class overload at zero-based197:42..60 in byte-exact0.6.3 sources. The
original broader oracle is preserved; the new oracle explicitly strengthens
the expectation to this single compiler-certified range. All guards remain
stable and the LSP closes normally. Evidence:
oracle120/typetools-editor-release19-strict1/{strict-audit.json,receipt.json}.

Delivered commits759bc25,9ab41b9 and3fb5afe fix diagnostic lock order, Java
class literals and same-method generic bounds respectively. Class-literal
tests verify scalar boxing, void, array rank, direct receivers, comments and
invalid/expect targets; existing alias rejection is reused. Bound tests cover
equal types, a distinct valid subtype, incompatible arguments and unsupported
scope/cycle shapes. Cache v41 refreshes the changed parsed bound facts.

Commit8f65812 shares bounded reference-overload selection between Java and
Kotlin. Actual argument substitutions prove applicability; declaration-level
shapes prove dominance. Unknown arguments, cyclic/incomplete hierarchy proofs,
unsupported conversion phases and generic competitors retain uncertainty.
Known applicable singletons keep their fast path. This does not establish
full overload fidelity or binary-only library selection coverage.

Independent review caught and repaired an intermediate checkpoint18 error:
instantiating generic formals before ranking wrongly selected one method for
`<T>pick(List<T>,T)` versus `pick(Collection<String>,Object)` and
`<T>pick(List<T>,List<T>)` versus `pick(Collection<String>,Collection<String>)`,
using (List<String>,String) and (List<String>,List<String>) respectively.
Javac reports ambiguity. Evidence:
oracle120/specificity-review-counterexample1 and
oracle120/specificity-review-nested-counterexample1. Both Java ambiguity cases
are retained in the shared regression and pass on19. Kotlin2.4 independently
selects the nongeneric overload in both reductions
(oracle120/kotlin-specificity-counterexamples1/receipt.json).
Reproducing that Kotlin choice is a separate unmet case; retaining multiple
candidates there must remain a coverage gap rather than a passing result.
Next: distinguish proven/disproven declaration forwarding from insufficient
proof before applying Kotlin's nongeneric tie-break. See the official
[MSC algorithm](https://kotlinlang.org/spec/overload-resolution.html#algorithm-of-msc-selection).
Contextual/trailing lambda inference, other unsupported type shapes and Java
nullability annotations also remain gaps. Warm optimization stays paused.

### Delivered Unknown fixes through120-22

Expected-type property lambda inference and lexical shadow/capture handling pass
focused tests and the actual editor. Source and compiled-class own-method F-bounds
preserve recursive nominal constraints; bounded-owner inference and actual CEM
getValue now return useful members. Cache v42 invalidated the changed bound facts.

Qualified classifier receivers now require a complete, unshadowed value namespace.
After-dot recovery borrows original host proofs only when every declaration fact,
path and tier agrees. Missing class, anonymous object and unmodeled lambda
receivers retain their presence; ordinary nonreceiver lambdas remain receiver-free.
Alias-backed receiver function types use the existing shared expansion. Cache v43
invalidates changed function receiver-role facts. Focused proof:
`cem-fqn125-joint-green3`,157 passing tests across7 stages.

Actual21 exposed two further shared gaps: Java/Kotlin CharSequence identity made
stdlib extension receivers Unknown, and implicit Object/Any compatibility caused
a false diagnostic after selecting the correct overload. Shared builtin identity
and reference-root compatibility now preserve nullability, primitive conversion
uncertainty and explicit/local foreign-name precedence. Actual Gradle JavaCompilerArgumentsBuilder
already emits effective -proc:none for CEM; source namespace authority now reuses
the same captured-option validator as compiler replay to recognize that fact.
Processor/plugin/conflicting arguments and stale classpath guards remain intact.
Focused proof: `cem-fqn126-green2`,13 tests across9 stages; first REDs retained.

Failed editor checkpoints20 (423/427, CEM4/8) and21 (424/427, CEM6/8) remain under
their original artifact names. The compiler-certified unique goto and original
FQN source oracles were preserved through the actual22 GREEN above.

### Previous integrated checkpoint120-17

Joint120-17 passes1776 Rust tests,0 fail,9 ignored (42 targets), and release;
575 input guards stay unchanged and all processes close normally. Its release SHA
is 53f09f28324356e5f7c5ec84fe71d2f4e16a17942bd01c2134aeec7cd0e2bcef.
The unchanged durable `java` scenario passes403/403 on this release, including
all28 new inferred-type assertions. The actual Java auto-import edit matches
the complete expected source; nullable function safe-call completion is useful.
The46 member probes each use one RPC. Inputs stay unchanged and shutdown is
normal. Evidence: native-integrated120-17/{workspace-summary.json,receipt.json}
and java-unknown121-release17/{summary.json,receipt.json}.

Source commit0d27151 also passes the original generic21, function-property10,
inferred-method4 and array12 checks on release17, all on first responses with
unchanged original expectations and normal shutdown. The harness's approved
fixture-only revision is recorded separately from unchanged project/manifest
guards. Evidence: cem-startup120/coverage/native-generic-inferred-release17,
function-property121-release17, function-return121-release17 and
array-property120/gradle-release17/receipt.json.

The same release now passes CEM26 in a controlled serial replay and all50
five-project requests: Gradle9, Moshi9, OkHttp10, Retrofit12 and Spring10.
All ten corpus stages and both CEM stages close normally with stable input
guards, no forced cleanup and zero owned residuals. Each useful RPC is a first
response; CEM's actual Float inlay response and exact goto token ranges are
audited separately. Corpus normal stages seed only the original project model;
each incomplete stage reuses its own cache. Evidence:
cem-startup120/editor-original26-release17-serial1/{summary.json,goto-audit.json,
replay-policy.json,receipt.json} and final-corpus120-17/summary50.json.

An earlier parallel CEM run on this same binary remains a failure:6 matched,
3 unmeasured and the incomplete stage unrun. Its K2 Java diagnostic producer
expired and its Float inlay request was unanswered. The trace records
compiler.java.editor-unavailable with stale-kotlin-source; no panic or process
leak occurred. The serial replay changes no expectations or request deadlines,
but its success does not establish the cause or fix this availability risk.
Evidence: cem-startup120/editor-original26-release17. These functional runs
make no new timing claim and do not establish general IDEA parity.

The shared type facts now distinguish Kotlin expect declarations, including
enclosing expect types, from JVM-nameable ordinary/internal/value classes.
Java scope completion excludes expect declarations and typealiases before
visibility conversion and name deduplication; Kotlin completion retains them.
Cache v40 invalidates every persisted symbol payload path; project files are
reparsed. The independent inline-versus-boxed serialization test includes the
new field and retains exact JSON, length and bincode comparisons. The earlier
release15 wrong-import snapshot and full16 obsolete-reference failure remain
recorded, not counted as successes.

Null literals use Nothing?. Source-Java and decoded-callable reference nullness
is still unproven, so only call-bound arguments retain the former uncertainty;
Kotlin nonnull and Java primitive negatives stay rejected. Independent Kotlin
and Javac controls validate both sides. Ordinary invalid calls can retain a
declared return for editing recovery while still producing the required error;
no null-only recovery policy was added. Evidence: java-autoimport121-expect-green3,
function-null121-recovery-green1 and its recovery-policy-review.json.
Java nullability annotations, contextual lambda inference and other unproved
type shapes remain gaps. Warm optimization remains paused per user priority.

## Integrated checkpoint120-9

Checkpoint source through2167820 passes1755 Rust tests,0 fail,9 ignored (40 targets), and a
release build. Both stages close normally with573 guarded inputs unchanged and
zero owned residuals. Evidence: native-integrated120-9/{workspace-summary.json,
receipt.json,main-ready.json}. The immutable release SHA256 is
6e21a28d2f85669183a411204512086e6a872c93c7ab775ee2bfdeae217a96d7.

Deferred Kotlin property accessors now obtain their types through shared,
current-source inference. Parsed facts retain no inferred result; cache v38
invalidates the changed positional schema. Nested source registration, unsaved
initializer edits, unknown types and cycles have focused regressions. Independent
Kotlin/Javac controls verify both the getter result and setter argument type.
On release120-9, the stricter real-editor slice passes9 cases/20 semantic checks,
all18 RPCs on their first response. Explicit excluded-candidate assertions verify
that unknown/cyclic types withdraw getValue while a same-phase getTitle control
remains useful. All23 inputs stay unchanged; shutdown is normal with no forced
cleanup or owned residuals. Evidence: cem-startup120/inferred-summary.json.
The earlier availability-only negative assertions are retained but are not used
to prove candidate absence.

The same release also passes original CEM26 (11 actual first responses, including
the Float inlay hint) and the original five-project50: Gradle9, Moshi9, OkHttp10,
Retrofit12 and Spring10. All ten corpus stages and both CEM stages shut down
normally, with unchanged guarded inputs and zero owned residuals. CEM goto
locations are checked against real, pinned source contents and token ranges.
The five-project normal stages start with genuinely cached project models but
no derived symbol caches; each incomplete stage reuses its own run's cache. No
timing claim is drawn from these concurrent functional runs. Evidence:
cem-startup120/editor-original26-release9/{summary.json,goto-audit.json} and
final-corpus120/summary50.json.

The matching IDEA oracle is now complete for those50 selected requests by
combining41 non-Gradle observations with the latest9 Gradle observations. Case
identities, manifests and the ILS263.6379.0/plugin9cfad180 pins match; overlapping
older Gradle results are replaced rather than double-counted. All observations
are first-response matches. This is not a single operationally successful IDEA
run: its shutdown failures remain recorded. CEM's property-form auxiliary and
the earlier method-form failures are separate. Evidence:
processor120/idea50-composed-audit.json. Selected-case agreement does not establish
general IDEA parity.

Mixed property/accessor rename passes14/14 checks in real Neovim on the joint
dev9 binary (SHA7d84c1dbcf87fcfa3b6cd088df0e5a6c81739032bc21047149f4425dc13e4c6e).
The actual WorkspaceEdit updates all three complete Kotlin/Java buffers, including
an aliased constructor argument and inherited getter use, preserves an unrelated
owner, and resolves the renamed getter to the new declaration before restoring
the buffers. Shutdown is normal and input hashes remain unchanged. Evidence:
rename120/{native-dev9-summary.json,editor-dev9/receipt.json}. Property override
families and ambiguous constructor families still refuse partial rename; this
does not establish full refactoring parity.

An external SDK consumer has been rebuilt against this exact source. Its composed
server passes the unchanged50 first requests and98/98 checks, including applied
rename, unsaved edits and configuration invalidation. The real Mixin JAR and
earlier independent compiler control remain bound to the evidence. Build and
editor processes close normally with unchanged guards and zero owned residuals.
Evidence: sdk-final120-9/{verified-summary.json,closed-api-summary.json}.
This is external-extension evidence, not production Mixin support in ktlsp.

JDK cache planning now runs within the bounded dependency worker pool rather than
blocking all useful module work. A gated regression verifies useful navigation
while planning is pending and rejects results from an obsolete generation;
scoped workers join before their owner returns. Quiet release120-9 records warm
first use at593.061..1177.902ms across five starts (p50=601.994ms,
p95=1062.820ms,500ms polling). Four of five are under1s; consistent subsecond
startup is not established. Thirty useful RPCs have p95=18.577ms, and menu handoff
p95=52.326ms. All31 actual insertions and all original8 checks pass with unchanged
inputs and normal shutdown. Evidence: cem-speed120-9/{performance-summary.json,
warm-startup-n5.json,budget-guard-audit.json}.

The latest user priority accepts this warm speed and moves valid-code Unknown
coverage ahead of further warm optimization. Current investigations separate
inferred generic properties, inferred method returns, function-valued properties
and arrays; expected types must be verified independently before claiming gaps or
fixes. Never replace honest unknowns with invented types. TypeTools also has a
reproduced POM/source-policy coverage gap while its binary API remains indexed;
the encoding-policy fix is verified separately below. These pending type changes are not
covered by the checkpoint's1755-test result until the next joint verification.

## TypeTools source policy and next overload gap

The real TypeTools0.6.3 POM's numeric compilerArguments source/target properties
now use the existing bounded expansion and encoding-closure guards. The reduced
real parent/child POM goes RED to GREEN and all15 existing encoding-policy tests
pass. Unknown compiler identities, argument files, injected options, ambiguous or
conditional properties and merge attributes remain rejected. Evidence:
oracle120/typetools-policy-{red1,regression1}/receipt.json.

Immutable dev10 (SHA16832674ec6b0096128e4270fafbf17d143de91c05110995b937b8d7b33003b2)
resolves both actual TypeTools source files without the former policy failure.
Java/Kotlin completion both pass. Every returned definition file matches the
selected source-jar entry byte-for-byte, but strict unique goto fails because both
overloads are returned. Preserve that failure: it is not a fully passing editor
run. Both exact consumers independently compile and javap selects only
`resolveRawArgument:(Ljava/lang/Class;Ljava/lang/Class;)Ljava/lang/Class;`.
Java class-literal typing and shared overload specificity are the next reproduced
semantic gap. All compiler/editor processes close normally with unchanged guards.
Evidence: oracle120/typetools-editor-dev10-1/audit.json,
oracle120/typetools-consumer-oracle1/receipt.json and
oracle120/typetools-selection-readonly1/audit.json.

## Pending Unknown coverage integration

Joint120-11 found5 new failures; joint120-12 then found27 existing regressions
from bypassing recovery and special super semantics. Restoring the original
completion route and allowing only exact unchanged recovered declarations to
use the current-source provider repairs all92 focused cases. The provider still
validates source identity; recovery does not gain property-access or ABI proofs.
A separate regression rejects invented zero-argument types for contextual lambdas.

Joint120-13 passes1770 tests,0 fail,9 ignored (41 targets), and builds release.
Both stages close normally with574 guarded inputs unchanged and zero owned
residuals. Immutable release SHA256:
ad26e929e4d1d784bb946903e7ef8b859530ff6cc6a15b1973a10dd5b9b6b26b.
Evidence: native-integrated120-13/{workspace-summary.json,receipt.json,main-ready.json}.
Real-Neovim original RED replays on this binary now pass method-return4/4,
function-property10/10 and generic-property6/6. Generic unsaved-edit, withdrawal
and restoration controls add15 semantic checks; all18 generic RPCs match on
their first response. These runs preserve inputs and close normally. Evidence:
function-return121-release13, function-property121-release13, and
cem-startup120/coverage/native-generic-inferred-release13/receipt.json.

The unchanged array replay initially passes10/12 on release13. Its2 Java
reference-array element failures expose an obsolete compiled-owner-only filter
discarding the already-correct source projection. Removing that filter repairs
the actual Workspace regression and all12 original editor assertions on immutable
dev14 (SHAa237d9bfe265ecd1e2299b62567d51eaff8158b306b7d4109ce52f2e4629863e).
All12 first requests match, inputs stay unchanged and shutdown is normal.
Evidence: array-property120/{gradle-release13,focus-source-array-red,
focus-source-array-green,gradle-dev14}. Final release replay waits for the
subsequent Java scenario fixes above rather than rerunning every intermediate build.

Original CEM26 also passes on release13, including all11 single-attempt responses,
the compiler-validated Float inlay and exact pinned goto locations. Both stages
close normally with441 guarded inputs unchanged. Source coverage now reports
zero failed and two missing dependency sources, compared with the earlier one
failed and three missing. This is a selected functional sweep, not new timing
evidence. Evidence: cem-startup120/editor-original26-release13/{summary.json,goto-audit.json}.

## Earlier integrated checkpoint120-6

Source through6619400 is pushed. The current full workspace passes1746 tests,
0 fail,9 are ignored (40 targets). Release compilation also passes. Both stages
close normally with573 guarded inputs unchanged, no forced cleanup and no owned
residuals. Evidence: native-integrated120-6/{workspace-summary.json,receipt.json,
main-ready.json}. The immutable release SHA256 is
2a02365ab3eb974941f21f9d73f9e675cf92e4dc9eb8bf3129d1e7eba381e158.

| Slice | Current evidence | Remaining limit |
| --- | --- | --- |
| File-local Java recovery | Pushed91d8145; counted regression and74 existing checks pass; original quiet Retrofit handler1689.496ms ->6.613ms, useful first response retained | Each timing is n=1; no general p95 claim |
| Plugin API beside sibling errors | Pusheda99cfe1; joint dev88b72502 passes15/15 checks, retaining original errors and rejecting invalid producers | Selected Noarg/serializer cases, not arbitrary plugin parity |
| Kotlin properties called from Java | Pushed6619400; actual stdlib expect/actual scalar boundary repaired;84 focused tests and17/17 editor checks pass, all16 RPCs first response on devb86293a6 | Inferred property types and derived-name rename still open |
| External semantic SDK | Freshly linked joint88 source passes unchanged50 first requests/98 checks | This precedes the later scalar-ABI guard adjustment; final broad SDK sweep remains separate |
| CEM / five additional projects | Earlier release18938 passes original26 /50 checks; lazy-binary devfe0977b6 also passes original CEM26 | Same-release final sweep and IDEA comparison remain open |

Quiet release120-6 now observes CEM first useful completion at35.969s cold and
1.700s warm (n=1 each,500ms polling). Thirty warm RPCs have p50/p95/max of
10.063/17.046/21.133ms; input-to-menu handoff is43.158/51.228/54.600ms. All31 UI
samples accept actual text, restore the whole buffer and leave disk unchanged.
The original8 checks match in each run;439 guards stay unchanged and shutdown is
normal. Source coverage still has one failed and three missing lookups. Cold30s
is missed; warm3s/RPC p95 50ms/menu p95 100ms are met in these finite observations.
Earlier same-policy observations were52.547s/1.689s cold/warm; the binary Create
preview falls from17.367s to1.284s, while Gradle dump now takes29.279s and remains
the dominant cold cost. No startup p95 or universal speedup is established.
Evidence: cem-speed120-6/{performance-summary.json,sessions-receipt.json}.

The user subsequently lowered cold startup's priority and requested warm startup
under1s. Thus1.700s is still an improvement target, despite meeting the former3s
budget; cold30s is a lower-priority aspiration. Next speed work profiles warm
startup's actual critical path rather than treating a faster request RPC as a
substitute for startup.

The budget document's observed-result paragraph changed between preparation and
the actual run, before its input capture; both runs' before/after hashes match.
The target table and comparison conditions are byte-identical. Evidence:
cem-speed120-6/budget-guard-audit.json. The quiet Retrofit replay also passes with
unchanged guards and normal shutdown:
oracle120/retrofit1752-release6-quiet1/receipt.json. Next: inferred-getter shared
inference, warm startup under1s and the remaining IDEA probes. Keep
compiler/control failures and native REDs below.

## IDEA follow-up120

The original Gradle9 cases now all match on their first request, including the
constructor completion, five AtomicInteger cases and the negative whose known
receiver control now succeeds. The engine is the pinned ILS-263.6379.0 server;
execution-authority.json binds its process image/job, launcher/plugin/import
hashes and initialize response. The old ktlsp binary field in the shared receipt
is unused bookkeeping, not the executed server. Original receipts are preserved.
Both harness stages remain operationally incomplete because shutdown requested
forced termination. Evidence:
idea-followup120/gradle1/{qualified-summary.json,execution-authority.json}.

The separate CEM property-form auxiliary case also completes one actual insertion:
raw breakSpeed item, resolve/apply command and workspace edit produce exactly
`return super.breakSpeed * (efficiencyLevel + 1)`, with the expected whole-buffer
hash and unchanged disk. This is one new request, not a replacement for the
preserved failed31-sample getBreakSpeed-method expectation. The native ILS exits0;
an exact-identity RTK tail wrapper needs cleanup, and the run is still classified
as an operational failure. All owned handles/processes are closed. Evidence:
idea-followup120/cem-property1/{verified-summary.json,rtk-root-operational-termination.json}.

## Earlier120 integration queue

At this earlier point, pushed source was91d8145. The workspace/release checkpoint below
predates the subsequent getter-body, plugin-sibling and lazy-binary changes;
its 1740 passing tests must not be attributed to today's pending source.

Retrofit's original incomplete completion takes 1689.496ms even in a quiet replay
of release18938. Existing trace instrumentation isolates receiver resolution:
RecoveredTypeIndex inherits entries_for_file's whole-index fallback, causing two
global scans for file-local type lookup. The targeted adapter fix and counted
regression now pass, together with74 existing regressions, and are pushed in
91d8145. Temporary timing spans are removed. No post-fix runtime improvement is
claimed yet.
Evidence: oracle120/retrofit1752-quiet1/receipt.json and phase-build3 artifacts.

A further plugin case exposes a real distinction between class output and typed
facts: a semantic error in a sibling top-level function leaves Model's actual
serializer bytecode usable by javac, but the file-wide error guard drops Model's
typed API. The class-level eligibility fix passes external compiler controls,
retains the original MissingName diagnostic and still rejects an erroneous
class's own initializer. The old binary's editor RED closes with the original12
checks passing, the added diagnostic passing and only the two new completion
checks failing (13 pass,2 fail). Joint dev88b72502 now passes all15 checks with
current diagnostics, unchanged guards, clean exit and no owned residuals.
Evidence: processor120/noarg-semantic-controls.json and
processor120/noarg-semantic-slice-summary.json. The unchanged external SDK's
50 first requests/98 checks also pass when freshly linked from this source:
sdk-final120-2/verified-summary.json.

Java setter projection is also pending integration. Actual Kotlin/javac controls
establish generic parameter types, void results, naming and private-setter
visibility. Focused84 tests pass, but actual editor validation exposes missing
concrete scalar setter candidates while the generic setter succeeds. This slice
is not accepted; the editor RED is retained in
cem-startup120/coverage/native-setter-fixed1/receipt.json.

The next complete workspace attempt (native-integrated120-5) closes naturally
with exit101 after86.828s and stable573 inputs. Its two failures are obsolete
assumptions about eager binary files and class-member facts on extension
functions; their original invariants are being updated to the new contracts.
The release stage does not run. Do not present the joint dev as a fully passing
workspace/release checkpoint. Inferred property getters also have an independent
compiler-positive/editor-negative reproduction in cem-inferred120/receipt.json;
they need current-source lazy inference rather than cached inferred parse facts.

The two test-contract repairs pass their focused runs (legacy cache1 and source
facts19). One earlier focused runner omitted an explicit KTLSP_CACHE_DIR; its
inherited value was not recorded, so actual default-cache effects are unknown.
The wrapper now pins and records a dedicated cache, and the repaired cache test
passes there. User cache contents were neither inspected nor cleaned. Evidence:
cem-speed120/cache-environment-audit.json. Editor runs already use isolated caches.

## Latest verified120: getter and configured SDK coverage

The joint120-4 checkpoint passes the complete Rust workspace:1740 passed,
0 failed,9 ignored. The release build succeeds with569 guarded inputs unchanged
and no forced or owned residual processes. Its immutable binary SHA256 is
18938e7be4e847ad687e4c1c64559d48a19beb80315f80c128f8b16522141b61.
Evidence: native-integrated120-4/{workspace-summary.json,receipt.json,main-ready.json}.
The preceding120-3 attempt failed to compile one old test caller after a function
gained a receiver argument; that caller is fixed. A subsequent formatting-only
change passes cargo fmt --all --check. These are build/correctness results,
not runtime speed acceptance.

Java constructor-property getters now share source definitions and receiver-bound
return types with the Kotlin property. Annotated library owners require the
selected classfile's actual getter descriptor and generic signature. Source cache
v35 invalidates entries that lack these facts. The original additional six library
cases now match12/12 first-response assertions on joint dev07fc12c2, with clean
exit and all36 semantic input guards stable. The raw guard result remains false:
two Gradle-generated probe/lock files changed, with no unexpected input change.
Evidence: cem-startup120/coverage/native-getter-fixed1/{summary.json,guard-audit.json}.
Mixed Java/Kotlin getter rename is rejected after an actual wrong-spelling edit
was reproduced; full derived-name rename and inherited getter coverage remain gaps.

An incomplete compiler recipe no longer withdraws another language's independently
verified SDK. For KGP1.9.25, configured SDK inputs are also captured before compiler
classpath resolution can fail; this narrow view does not certify compiler execution,
generated output or producer closure. The actual KGP toolchain/provider and runner
source confirm the selected JVM path. The actual11-case Gradle contract passes;
a separate build-JVM17/task-JVM21 contrast failed during listener setup and is not
claimed as verified. Evidence: spring120/{capture1,contract3}/receipt.json.

Spring now matches all10 original first responses on the same dev07fc12c2 binary:
normal4/4 and incomplete6/6, including only the expected ResolvableType overload.
Both sessions nevertheless require forced termination after the shutdown wait.
Their traces reach workspace.drop.start but not workspace.index.dropped; the
Gradle child had already exited. Index destruction is a candidate cause, not yet
a proven cause or a passing lifecycle result. Evidence: spring120/{native1,
native1-incomplete}/receipt.json.

Gradle's original nine-case run now closes with9/9 qualified first-response
matches, including the known positive control for its unknown-receiver negative.
Both normal3 and incomplete6 sessions close cleanly with stable inputs and no
owned residuals. Evidence: oracle120/gradle-sdk-native1/closed-audit.json.
The new normal Gradle import also resolves all compiler tasks successfully;
isolation of the per-language SDK fix therefore comes from the preserved old
captured-model RED/GREEN, not this recovered import alone. Across recorded runs,
all50 original five-project semantic expectations now match, using earlier
release120-2 for Moshi/OkHttp/Retrofit and joint dev07fc12c2 for Spring/Gradle.
This is not yet a same-release sweep or full-project semantic coverage. Final
release cold/warm/UI measurements and the Spring lifecycle result remain open.

The same joint120-4 release subsequently passes all50 original five-project
first responses: Gradle9, Moshi9, OkHttp10, Retrofit12 and Spring10. All ten
sessions close normally with stable inputs and no owned residuals. Spring's
Index destruction takes10.212s/2.754s in these functional runs; passing lifecycle
does not establish a startup/completion speed target. Evidence:
oracle120/release120-4-corpus40-summary.json and spring120/release1-native10-summary.json.
One Retrofit incomplete completion takes1.753s; its cause remains under review.

Quiet CEM measurements on that release observe useful completion at52.547s cold
and1.689s warm (n=1 each,500ms polling). The cold useful response precedes both
binary-preview and source-query completion. Warm30 RPC p50/p95 is10.421/16.547ms;
input-to-menu handoff is44.026/51.686ms, using linear interpolated quantiles.
All31 UI samples accept and inspect the edit, restore the whole buffer and leave
disk unchanged. Both sessions close normally;438 guards stay unchanged. Source
coverage still reports one failed lookup and three missing sources. Historical
113's Gradle artifact cache was less populated, so these observations do not
isolate a binary speedup or establish startup p95. Evidence:
cem-speed120/performance-summary.json and docs/fork-cem-startup120.md.

Inherited Java calls now obtain constructor-property getters from the existing
single hierarchy walk. Source definitions, declaring owner and generic receiver
bindings are preserved. Focused11+71 tests and four native first responses pass
on joint dev902417c9 with clean exit and stable inputs; the independent Kotlin/
Javac control compiles the unchanged fixture. Evidence:
cem-startup120/getter-inherited-summary.json. Class-body properties, open-property
coverage and mixed getter rename remain separate unfinished work.

A fresh separate extension consumer linked from that same source passes the
unchanged50 first requests/98 checks, including applied rename, unsaved buffers
and configuration invalidation. The upstream Mixin inputs match the previously
compiled control; this run does not recompile that upstream control or verify
game launch/remapping. Evidence: sdk-final120/verified-summary.json.
The first Noarg FULL-codegen filter improves the OpenBase same-file case but
still fails two serializer completions beside Model itself. The accepted current
compiler namespace is followed by NotIndexedDeclaration: the source extractor
incorrectly labels a top-level function as a class member. Normalization now
retains top-level and extension-function declarations without that invalid member
claim; the host validation contract stays unchanged. Actual FULL codegen filters
out erroneous package parts/classes while retaining current plugin output from
valid classes, original diagnostics and KAPT3 fallback. On joint devfe0977b6,
all12 checks now pass: the original six plus current canonical syntax diagnostics
and Java/Kotlin completion at both Model/OpenBase unsaved edit positions. Inputs
and hook hash stay unchanged; shutdown is clean with no owned residuals. The two
source-normalization tests, five profile tests and43 core extension tests pass.
Producer completeness and diagnostic absence are not certified. Evidence:
processor120/noarg-slice-summary.json and serialization120/editor-noarg2/receipt.json.

The same devfe0977b6 now passes12/12 native first responses for explicit-type
class-body/open/override properties, preserving nearest declarations and generic
return chains. The83 related Rust tests pass. Its parser cache is v36; inferred
property types, setters and mixed getter rename remain unfinished. Evidence:
cem-startup120/getter-body-summary.json.

Binary preview now parses the existing rendered headers directly, recording a
binary-specific marker instead of writing and rereading every stub. Navigation
materializes the selected file; valid repeated reads avoid regenerating it.
Changed/deleted/malformed identities reject cached facts and retract old text.
The renderer, descriptor/member inventory and declaration offsets are retained.
Related binary16/JAR29 tests pass, including byte-identical lazy goto, warm-cache
reuse, repeated-read mtime preservation and identity failures. On devfe0977b6,
the unchanged CEM main9/incomplete17 all pass with439 guards stable and clean
exit. Each cache has101 binary markers and zero eager binary Java files. Its
two final goto targets are actual source paths; binary-only navigation is proved
by the focused Rust cases rather than these two editor requests. Improvement in
cold time still needs a release measurement. Evidence: cem-speed120/lazy-result.json
and cem-speed120/editor-original26-lazy-dev1/summary.json.

## Earlier120: development builds and functional baselines

The joint120-2 integration passes the full workspace:1729 passed,0 failed,
8 ignored. Release build also passes;564 guarded inputs stay unchanged.
The copied release SHA256 is
a099aeee9084d54e277a81db8c765bb67053ef6256c044e19f5dbf14437d8f44.
The first integration exposed a duplicate source-hierarchy walk; the fix reuses
already collected candidates without changing the cycle-test expectation.
Evidence: native-integrated120-2/{workspace-summary.json,receipt.json,main-ready.json}.

With that release, serialization editor6 passes5/6 original first-response
checks: Kotlin golden/partial/dot and Java golden/partial pass. Java dangling-dot
still returns an unknown receiver and is the next focused fix. Actual backend
class output supplies Noarg's constructor; scoped private watches avoid overlay
self-invalidation. Language-aware parsing fixes Java source normalization and
preserves accepted Kotlin API through Java body edits. Shutdown is clean.
Evidence: serialization120/editor6/receipt.json and
serialization120/editor6-harness/artifacts/corpus-results.json. These selected
plugin results do not establish arbitrary compiler-plugin support.

The Java dangling-dot recovery fix now retains the original owner's independently
verified value-namespace proof when recovered declaration headers still match.
It does not promote that proof to complete source members. The actual FULL-output
host regression covers partial/dot with and without this proof; Root reruns it
successfully. All-workspace formatting also passes. Native editor7 now matches
all six original Java/Kotlin golden, partial and dangling-dot first responses,
with unchanged inputs, clean shutdown and no owned residuals. This uses the
15s incremental dev build, not a release speed comparison. Evidence:
serialization120/{dot-host-focused-receipt.json,root-dot-check.log,editor7/receipt.json}
and serialization120/editor7-harness/artifacts/corpus-results.json.

The release's six additional library cases pass5/6: Patchy and TypeTools both
languages, and Kotlin Pair.first. Java Pair.getFirst completion and goto both
fail despite actual Java compilation succeeding. The ordinary Kotlin constructor
property is indexed, but its Java getter projection is missing; this is a real
coverage gap, not evidence that missing source jars alone remove all library APIs.
Evidence: cem-startup120/coverage/native-release1/harness/artifacts/corpus-results.json.

Current-release five-project runs close all50 original first-response queries:
Moshi9/9, OkHttp10/10, Retrofit12/12, Spring9/10 and Gradle4/9 raw matches.
There are43 qualified matches, one unqualified negative whose known control
fails, six semantic mismatches and zero unmeasured requests. Spring goto includes
both Class<T> and ResolvableType overloads instead of the latter alone. Gradle's
five known AtomicInteger completion requests return null; the independent
compiler controls still establish the expected member. Every session closes
cleanly with stable inputs. LiveGradle's first run performs a successful phase0
build but reaches the harness's fixed120s build wait before any of its90 semantic
checks. It is operationally incomplete, not90 semantic failures. The harness
now waits for actual build exit; outer owned-process cleanup and semantic
expectations remain unchanged. The separate joint-l2 rerun now passes90/90,
including61 first requests and four actual Gradle builds, dependency/generated
source replacement and removal, preserved unsaved Kotlin edits and Java Unicode
diagnostic correction. Shutdown is clean; no forced or owned residuals remain.
Evidence: oracle120/joint120-summary.json, joint-l2/receipt.json and
C:/Users/gummy/.codex/ktlsp-oracle120/n/l2/harness/artifacts/gradle-reload.json.

The current-release CEM main and incomplete sessions also pass all26 assertions
on11 first requests, with439 input guards stable, clean exit and no forced
termination. Source coverage still records three missing sources and one failed
source lookup; the additional library probes above distinguish API coverage.
Evidence: cem-startup120/editor-original26-release1/summary.json.

The first processor54 release run reaches49 checks, one missing Java obsolete-
API diagnostic and four unrun Kotlin checks. Its cold observer script and
configuration consume94s before the26s producer DAG, then observation-window
shutdown cancels the producer. Reusing the populated observer cache, the same
release now passes54/54 and the unchanged original114/114, including disabled
and restored generation, both languages' obsolete-API diagnostics and current
canonical observations. Both runs close naturally with stable inputs and no
owned residuals. The Java diagnostic issue does not reproduce once the current
observation is available; no semantic guard is relaxed. Global producer closure
remains unproved, and cold preparation latency remains a performance gap.
Evidence: processor120/acceptance-release120-2.json and
processor120/original114-release120-2-warm/run/artifacts/processor-reload.json.

An actual Spring buildSrc replica reproduces model imports cancelling themselves
when Gradle recreates its JAR (the17 file contents stay identical; ZIP timestamps
change). During an owned model load, JAR notifications now coalesce into a pending
forced refresh without killing the dump. Its unsettled generation is discarded;
only the subsequent model is installed and persisted. Configuration changes and
mixed batches retain immediate cancellation. The existing singleflight test and
cold native replica verify actual changes/reload, two useful first responses,
stable inputs and clean exit. An earlier candidate's shutdown timeout remains
recorded; this is not a release speed comparison or full Spring acceptance.
Evidence: project-refresh120/result-summary.json and project-refresh120/green-native2/receipt.json.

Commit a886d2b reduces dev debug information while retaining workspace panic
file/line data. Release optimization and linker settings are unchanged. Three
local edit/test samples have medians5.819s before and4.646s after; the same
profile change alone does not establish a server build speedup. A separate
Windows final-binary linker comparison measures13.784s with MSVC versus12.810s
with the bundled LLD, each n=3. Use the documented dev command rather than
rebuilding release during focused iteration. Root reruns the five selected core
tests successfully and verifies both native-editor basic summaries/assertions.
See docs/build-performance.md and build-speed120/summary.json for method,
raw evidence and limitations; cold-build and release runtime gains are unproved.

The four Java Holder.ITEM chain failures are fixed. An unbuilt Kotlin output
directory was treated as unknown compiler-service inventory, invalidating the
Java caller's value-namespace proof. NotFound now proves current absence without
caching it; later directory/Jar services and classpath changes retract the proof.
Other IO failures and invalid archives remain unknown. Five related tests pass
after the focused regression fails before the fix; an independent javac17
control compiles the same static chain. The unchanged original19-case editor
run now matches66/66 checks, with no failed/unmeasured result, clean shutdown
and unchanged buffers. Root verifies the raw report and snapshot provenance.
Evidence: binary-static120/result-summary.json and binary-static120/joint/receipt.json; binary
SHA256 a3df1df10c54f99f2c04de2f791805745ebc17d2625d87e7a1974c120ed24c39.

The processor snapshot now accepts a source read only when its declared generated
output root and exact earlier closed output SHA establish current ownership.
This covers KAPT stubs without adding them as authored/editor sources. Focused
tests pass24/0 with one existing ignored test; Root reruns the new owner test.
The real dev phase54 closes47/54, passing the previous owner boundary and saving
all26 events. Its remaining checks exceed the120s observation window; shutdown
then cancels the active Java-helper work and produces stale-kotlin-source.
The observed stale token is not evidence of an independent source-lifetime bug.
Release validation and the original114 remain pending. Evidence:
processor120/phase54-2/{receipt.json,lifecycle-timeline.json}.

The real serialization fixture uses Gradle8.10.2, not the initially assumed8.7.
Its actual distribution ABI and upstream source establish a separate Java17/
release17 processor-enabled metadata adapter. Twenty capture/negative checks
pass with unchanged project/wrapper inputs and no remaining owned processes.
Custom task actions/predicates remain unproved; metadata alone grants neither
processor API nor absence authority. Evidence:
processor120/java8102-model1/receipt.json and processor120/java8102-abi/.

Serialization's earlier editor3 passes the original Kotlin golden, partial and dangling-dot
completion cases on the first response (3/3). The original Java cases still
return unknown receivers (0/3); the real Gradle/Lombok caller needs an actual
javac namespace witness rather than the plain-Java absence proof. Positive
root/child class facts share extension readsets and callable selection; the
generic-result tests pass. The earlier observer-directory collision is fixed.
Evidence: serialization120/editor3/receipt.json and
serialization120/editor3-harness/artifacts/corpus-results.json. All six editor
requests use one attempt; Kotlin success does not establish Java acceptance.
The shared5 integration then regresses to0/6 (six received first-response
mismatches, zero unmeasured); Root verifies its raw report. Actual javac rejects
the observed Kotlin headers because Noarg's real generated NeedsLabel()
constructor is absent from the KAPT3 header output. No Java namespace authority
is published. The code retains independently verified Kotlin positives when the
Java side fails; it does not discard them merely for a Java error. Another
observed invalidation occurs when long-lived input watches include owned overlay
lombok.config paths: removing the temporary overlay invalidates its own accepted
environment before publication. The rejection precedes shutdown. Scoped private
watches and actual backend generation for the selected compiler plugins were
subsequently implemented; see editor6 above. Cold model/JDK rebuilding is not warm timing evidence. Shutdown also fails to finish after its
response/exit notification. Evidence: serialization120/editor4/receipt.json and
serialization120/editor4-harness/artifacts/corpus-results.json.

The separately built extension consumer passes4/4 Rust tests with the current
host/core and a synchronized lockfile (19 added lines; existing versions stay
unchanged). One real-Neovim composed-provider case using the pinned upstream
Mixin/Shadow declarations matches98 semantic checks across50 first-attempt RPCs.
Rename edits are applied; unsaved/configuration changes retract prior facts.
Inputs stay unchanged, shutdown is clean and no owned processes remain. This
is a dev correctness result, not release performance or production Mixin/game
acceptance. Shape-only editor extras remain unrun. Evidence:
sdk120/{result-summary.json,native-receipt.json,upstream-inputs.json} and
sdk120/native/artifacts/corpus-results.json. Earlier SDK104's three cold
responses were three RPCs in one case, not three complete SDK runs.

CEM binary projection now retains supported Java interface default/static
methods and constants. A bounded shared completion proof preserves a concrete
zero-argument inherited method below a missing upper ancestor; unresolved
sibling/overload cases still stay unknown. The actual CEM/Create reduction
changes from RED to GREEN without source jars. Focused Rust checks pass15+67;
four new native-editor cases pass12 assertions after independent Gradle
compilation. A subsequent shared proof now resolves the nearest concrete
zero-argument declaration for completion, goto and Float inference on the actual
CEM buffers across local/module and cold/warm caches; the focused67 receiver
checks and two actual-Create checks pass. Exact selection retains its bounded
arity/generic/vararg guards. Source caches advance to v34 so old cached interface
constant omissions cannot persist. Explicit InnerClasses ownership also retains
the actual read/write nested parameters, with16 binary-Java tests passing and
three actual javac super calls compiling. Three native partial-completion
requests match on their first response, but that run needs forced shutdown and
is operationally incomplete. Full original CEM release validation remains
pending. The shared5 original CEM main run now passes its original eight checks
plus one new Float inlay assertion on five first-attempt RPCs. Root verifies its
raw report (9 matched, zero mismatched/unmeasured, clean shutdown). The new
unsaved subclass and observed CRLF wire buffer independently compile against
the actual111-entry Kotlin2.4/JDK21 classpath. Original incomplete probes also
pass:17/17 assertions, bringing the two CEM stages to26 assertions across11
first-attempt requests, with clean shutdown. These are shared5 dev results;
joint120-2 release CEM and additional-five-project reruns are now assigned.
Evidence: cem-startup120/editor-original19-1/cem-working-copy/artifacts/corpus-results.json
and cem-startup120/inlay-wire-control/receipt.json. See
docs/fork-cem-startup120.md for exact scope and evidence.

The shutdown problem came from cancelled dependency-index workers continuing
to parse and cache the stdlib. Existing generation checks now stop source
resolution and per-file parsing, without publishing a partial cache. The same
stdin-open reproduction changes from17.047s to125ms natural exit; the original
three native completion probes then pass with clean shutdown. These are focused
dev observations, not release runtime benchmarks. Evidence:
lifecycle120/{green2/receipt.json,summary.json} and
cem-startup120/parameter-native-fixed/receipt.json.

The additional-five-project IDEA oracle has closed all50 original requests:
43 qualified matches, one empty negative without its required positive control,
six unmeasured completion requests and zero measured semantic mismatches.
Gradle's two goto requests match; its six completions reach the unchanged5s
deadline and return cancellation, not useful late results. The empty unknown-
receiver result is not counted as a qualified pass. These IDEA-side deadlines
do not establish a ktlsp semantic failure or a whole-root parity result.
CEM's warm UI probe receives IDEA's synthetic-property
label breakSpeed (from getBreakSpeed()), failing the unchanged exact-method-label
expectation on its first cycle;30 later cycles are unrun. Cold import/index
readiness attempts are separately unmeasured, not semantic passes. No comparable
IDEA UI-speed result is established. Evidence: oracle120/closed-oracle-audit.json
and oracle120/c4/artifacts/completion-ui.json.

No acceptance expectations or first-response timing are relaxed. Remaining
processor/plugin, CEM startup/navigation and full-corpus gaps prevent claiming
the complete requested editing experience.

## Latest113: integrated owner types, navigation and Windows allocation

Root runs the combined workspace and release build with the Windows allocator
enabled by default. Final results are1,715 passing/zero failed/eight ignored
across40 test-result blocks; release succeeds. Root independently verifies
all545 current input hashes, identical before/after snapshots and three raw/
binary references. Both Jobs close with no live residuals after cleaning one
owned MSVC helper each. Evidence: native-integrated113-2/receipt.json SHA256
5f7dbef0bb72c3aea46af52c24ec232eb0e353f1441fe890d437496d3d3374c2.
The21,147,136-byte immutable binary is native-integrated113-2/bin/ktlsp.exe,
SHA25642ca19f87daf29329b6b3012210da197a4d9aa7187566a88c231475b3a4deeb5.

Nonstatic binary inner types retain separate enclosing and inner arguments.
Exact reciprocal InnerClasses records establish capture; missing/ambiguous
owners remain unknown. Typed classfile projection and Java/Kotlin result chains
pass the focused checks. Symcache identity changes to v33. The existing binary
editor scenario retains its original18 cases and adds one owner-chain case.
The real19-case run has62 matched and four mismatched checks: the new enclosing
owner case passes, while original Java Holder.ITEM completion/navigation checks
fail in normal and unfinished code. Kotlin's equivalent static chain passes.
Causality with the new owner implementation is not established. Evidence:
binary-generics113-native1/harness/artifacts/corpus-results.json.

Explicit Kotlin navigation no longer falls back from an unknown receiver to
an unrelated globally unique member. Its public goto reproduction fails before
the fix and passes after it, alongside the existing selector checks. The first
full run exposes an old test relying on undefined makeA() to find A.alpha();
the fixture now constructs A(), keeping the alpha target. Its old grammar-error
comment is corrected: the pinned parser reads both one-line classes normally.
The original full-run failure and focused checks are retained, not relabeled.

KAPT/KSP recipe checks now bind the full ordered primitive KAPT configuration
to each raw argument before rebasing, preserve native path spelling, and admit
only a proved later empty KAPT classes slot for the selected KSP task. All three
focused reproductions pass after failing before the fix. Native phase54 then
passes47/54, next rejecting processor-bootstrap-read-outside-current-overlay-closure.
The original114 is not rerun until this subset passes; Rust success does not
establish processor acceptance. Evidence:
recovery89/corpus90/native-processor-owner113-phase1-2/receipt.json.

The latest real serialization fixture compiles and emits Companion.serializer()
returning KSerializer<Model>, but all six original Java/Kotlin golden, partial
and dangling-dot completions mismatch on112, with first responses received and
no retry. Root verifies its raw/report references. The selected compiler also
rejects captured annotation -P options in the current adapter. An external
current-frontend observation finds the generated public synthetic method even
for partial/dot inputs; publishing these facts through shared analysis is next.
Evidence: serialization113/editor/receipt.json and
serialization113/frontend-probe/{valid,partial,dot}/receipt.json.

Old Temp corpus inputs disappear during preflight. All five checkouts are
recovered outside Temp at their pinned commits with67 original semantic buffer
hashes preserved. CEM's40 missing inputs match the user's read-only originals;
the recovered snapshot and118 dependency hashes match previous evidence. Ten
initially reported missing Gradle files are instead Windows long-path audit
false negatives; the extended-path audit records the correction. Bootstrap is
separate from timing. Current-binary CEM cold/warm checks pass8/8 each and UI
accepts31/31 cycles of one getBreakSpeed case. First observed useful completion
is170.189s cold and3.453s warm, each n=1. Warm30 RPC p50/p95 is15.234/32.586ms;
menu handoff is49.980/68.388ms, not physical display latency. The final dependency
inventories agree; full logical-symbol coverage equivalence is unproved. All
three sessions close without owned residuals. Evidence:
recovery89/cem-first-readiness113-1/performance-summary.json. The original CEM18
and whole additional-five-project acceptance still require the corrected build;
no final speed or parity is claimed.

## Previous112 investigation: processor input ownership and speed candidates

The generated-method diagnostic adapter no longer rejects Japanese receiver
and method identifiers merely for being non-ASCII. The focused reproduction
compiles and fails before the three-line change (zero admitted diagnostics
instead of one), then all five related checks pass. These include the existing
real-Javac observer with Japanese owner/current/missing method names, decoded
classfile facts, two observed calls and the exact UTF-16 error range. Root
verifies both raw output references. This extends the existing identifier
subset, not all Java Unicode identifier rules or generated-API completion.
Evidence: recovery89/processor-identifiers113/summary.json SHA256
00bd967b77222eba22dc5438798a4e9831d61fc7f6240f62e25debb4b1183780.
Each Cargo Job closes with no live residuals after cleaning one owned MSVC
vctip.exe helper; this is not a forced production-LSP shutdown. The combined
workspace/release and editor validation remain pending.

The targeted two-phase processor reproduction now preserves private events
before snapshot cleanup. Its26 events identify the previously rejected input
as the selected processor Jar task's default25-byte MANIFEST.MF. The112 candidate
admits that exact payload/path only when the same selected Jar task succeeds
and its modeled output matches the current inventory. The real invocation
passes this input check and next rejects differing compiler options. Root reads
the actual editor output:47 passes, three failed obsolete Java API diagnostics
and four unmeasured Kotlin diagnostic checks. The original114 remains RED;
this54-check diagnostic subset does not replace it. Evidence:
recovery89/corpus90/native-processor-owner112-phase1-1/receipt.json SHA256
09e22dfef51fbc80c29209e01b0ed8d98a0dafea739c044029c825ff4162a4f6.

The111 allocator trial completes warmups plus ABBA with two measured runs per
variant. Root verifies12 receipt/trace references; all six runs match the same
three first responses and close naturally without owned residuals. Medians
System/mimalloc: process CPU92.76/76.59s, index destruction18.65/4.21s,
post-index first-tested-useful47.05/55.11s, completion handler6.78/7.84ms.
Concurrent CPU activity and n=2 prevent a startup non-regression conclusion.
The initial ABBA does not justify adoption by itself; the follow-up comparison
and unsaved-edit checks are recorded below. Evidence:
recovery89/allocator-abba111-1/summary.json.
These requests follow WAIT_INDEX and do not measure earliest useful completion
or visible editor latency. Cold/warm index-count differences are explained by
cumulative preview/source counters, not demonstrated missing semantic facts;
full binary-only fact parity remains unverified.

The follow-up pair excludes other agent benchmark/build jobs while retaining
normal background processes. System/mimalloc post-index first-tested-useful
is46.893/42.082s, process CPU86.94/72.58s and completion handler6.143/6.479ms.
All three original first responses match; models are byte-identical and both
servers close naturally. Root verifies four receipt/trace references. The
existing Java scenario separately passes367 assertions for each allocator,
including unsaved cross-language edits. This supports retaining the candidate,
not a one-pair population speed estimate. The trial feature/System branches
are removed in favor of a Windows-only default allocator; its final combined
build is pending. Evidence: recovery89/allocator-quiet111-1/summary.json SHA256
9364870a7d4c5626633b7d73c3500e7a8ab0255a98e4ee3e5940969d30d5852a,
and recovery89/allocator-quiet111-java-summary.json.

The content-addressed Gradle init-script candidate has focused/classpath tests
and release111. CEM warmups plus ACCA complete with all eight original assertions
passing in each of six runs; Root verifies25 receipt/raw references. After
warmup, the candidate reuses the same four compiled Gradle script classes with
unchanged bytes/hashes/mtimes across both measured runs; the baseline creates
distinct timestamp-named classes for model and source queries. Measured
post-index first-tested-useful medians are119.10s/107.95s, but the candidate's
samples lie within the baseline range and background load differs. This proves
compilation reuse, not a causal speed win or a reliable tail latency estimate.
Evidence: recovery89/cem-init111-perf1/performance-summary.json SHA256
7e14d4d823280198caaa68d71ffb34a077c104237136b521fbde814115595ddc.
Earliest useful completion and persistent-cache reopen remain unmeasured.
Neither a full correctness rerun nor speed acceptance has been established
for the combined current edits.

## Previous110: selected KAPT runtime and warning-free Gradle import

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
