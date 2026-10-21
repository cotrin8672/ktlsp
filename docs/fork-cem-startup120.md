# CEM binary completion slice (checkpoint 120)

Baseline revision: `a886d2bdb19d1354a3d7284ee527657f6da48512`, with concurrent,
uncommitted fork work. Windows, Rust 1.98.1, Oracle JDK 21.0.3 for the pinned
classfile and JDK 17 for the disposable Gradle consumer; Neovim 0.12.5.

Local evidence root (`E` below):
`C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda/cem-startup120`.
Neither the user's CEM checkout nor its original sources were changed.

## Cause and boundary

The previous release113 CEM cold run first returned useful `getBreakSpeed`
completion at 170189.0335 ms (n=1). Its binary preview finished at 62.935 s;
Create's binary members were already installed. Its cached binary stub already
contained the protected method. Missing protected access was not the cause.

The Kotlin class has two direct supertypes. The shared super-member query
discarded every candidate if either provider's hierarchy was incomplete.
Create's upper `SyncedBlockEntity` remains unavailable because it carries class
annotations; an unresolved ancestor above the known concrete declaration thus
erased a useful positive completion below it. Four inspected Create interfaces
also became name-only placeholders: default methods were rejected wholesale,
and one unsupported nested parameter erased an otherwise known interface.

The renderer now preserves supported Java default/static methods and interface
constants. Unsupported members retain their names as unavailable facts; class
annotation guards remain intact. Interface constants reuse the ordinary field
extraction/flags path. Local/module binary symcache fingerprints advance to v8;
the shared source/JDK symbol-cache version advances to v34 so existing source
caches cannot silently omit interface constants.

The shared member layer retains a known concrete, nonstatic, nongeneric class
method if its class path to the
declaring owner is known and sibling interfaces have a complete, empty surface
for that name. Missing ancestors above that owner cannot erase the positive
fact. Completion preserves supported argument-taking signatures and overloads
from that owner. A nearer same-name declaration hides ancestor candidates in
this bounded fallback; inherited overload families still need broader proof.
Exact navigation and call selection additionally require one fixed zero-argument
declaration at that owner. They select the same declaration and return type as
completion. Unknown sibling kinds, competing interface names, intermediate gaps,
nearer abstract/unsupported declarations, generics and varargs remain unknown.
The result still reports `query-scope-incomplete`; selecting this exact known
declaration does not certify the whole hierarchy.

## Acceptance record

| Case / expected result | Observed result and status | Evidence / remaining gap |
| --- | --- | --- |
| Actual Create four interfaces retain interface identity and supported default methods; nested unsupported member stays unavailable | RED 0/4 identities, then GREEN 4/4 | `E/actual-interfaces-receipt.json`, `actual-interfaces-{red,green}.log`, `actual-interfaces-javap.txt`; class annotations remain guarded |
| Exact two Create classfiles plus unchanged CEM two Kotlin buffers yield useful inherited protected completion before upper ancestry is available | RED exit101, GREEN exit0; one `getBreakSpeed` candidate, query remains Unknown | `E/actual-reduced-receipt.json`, `actual-reduced-{red,green}.log`; reduced semantic proof, not full-CEM startup |
| Known class completion survives a missing upper ancestor; nearest declaration wins; unsafe exact variants remain unknown; source replacement invalidates/restores it | GREEN, 11 variants; known/nearest goto and Float agree | `implicit_receiver_inference::super_known_class_members_above_missing_ancestor`, `E/implicit-shared-exact.log` |
| Three actual Create classfiles plus unchanged original two CEM buffers: getBreakSpeed completion, exact goto and Float inference; addToGoggleTooltip/read/write positive completion | Goto RED empty, then GREEN in local/module, cold/warm cache paths; 12 parameter-completion assertions also GREEN | `binary_java_actual_create_hierarchy_survives_local_module_and_v2_cache`; `E/parameter-control/rust-actual-green.log`; real javac super-call control independently passes |
| An old v33 source symbol cache omits interface LIMIT; current cold/warm source parsing retains its type and implicit public/static flags | GREEN | `source_interface_constant_rejects_pre_constant_symbol_cache`, `E/cache-v34.log`; shared version also invalidates JDK-source fingerprints |
| Separate abstract Kotlin subclass uses real Drill constructor/interface and infers local speed from super.getBreakSpeed | Independent Kotlin 2.4/JDK21 compile GREEN; standard-LSP first inlay response GREEN at zero-based line9/character17, label `: Float` | `E/inlay-control/receipt.json`, `E/inlay-wire-control/receipt.json`, `E/editor-original19-1/cem-working-copy/artifacts/cem-super-inference.json`; inherited DOS fileformat changes only line endings, and the exact wire body also compiles |
| Binary/default/partial-interface, cold/warm cache, protected access, generic, static/private and unsupported-name checks | 15/15 GREEN | `E/rust-final/receipt.json`, `binary-java.raw` |
| Existing implicit/super receiver behavior and shared bounded declaration proof | 67/67 GREEN | `E/implicit-shared-exact.log`; earlier completion-only checkpoint in `E/rust-final/receipt.json` |
| Pinned default-interface fixture and its Java/Kotlin consumers compile independently | GREEN, Gradle `classes --offline` | `E/compiler-control2/receipt.json`; initial public Kotlin/package-private Java fixture visibility error was corrected by making the new Kotlin fixture private |
| Four real-Neovim Java/Kotlin completion probes return useful protected/default methods on the first response and exclude static/private names | GREEN: four first responses, 12 assertions; guards stable, EOF, zero live owned processes | `E/editor-defaults3/receipt.json` and `harness/artifacts/summary.txt`; one deliberately source-less binary module remains reported as missing source |
| Full release CEM cold/warm/UI and broad workspace checks | Selected original eight checks pass in both timing sessions; UI31 accepted; workspace1740 passed,9 ignored | `cem-speed120/performance-summary.json`; cache-state and coverage qualifications below, no IDEA-parity claim |
| Integrated dev original CEM main and incomplete cases plus one inference check | Main9/9 and incomplete17/17 semantic assertions GREEN; ten original corpus requests plus one inlay all first-response matched. Both native exit0, EOF, live0, no forced termination, 439 guards stable each | `E/editor-original19-1/summary.json`, `receipt.json`; original18 accounting counts ten corpus requests plus eight K2 checks, while actual assertions also include exclusions/controls |

The disposable binary fixture now includes `defaultfixture/Overlay.class`, so its
jar hash changes. Original binary12/fields2/generics19 buffers and expectations
are unchanged. The first new editor attempt stopped at its input hash guard:
the new manifest initially used a terminal-newline digest rather than the
harness's established normalized-lines digest. No semantic expectation was
relaxed; the final generator checks all four new digests before writing a project.

## Reproduce / next action

```sh
rtk cargo test -p ktlsp --lib binary_java_
rtk cargo test -p ktlsp --test implicit_receiver_inference
rtk proxy python3 dev/binary_java_jar_project.py NEW_TEMP_DIRECTORY module
rtk proxy bash dev/ktlsp-harness.sh corpus --root PRINTED_PROJECT \
  --file dev/corpus/binary-java-defaults.json --no-build --bin CURRENT_BINARY
```

Use the bundled absolute Python path on this Windows host: the ordinary Python
alias is unavailable. Preserve an explicit private `KTLSP_CACHE_DIR` and use the
existing owned runner for native child cleanup. The final editor receipt confirms
source/binary guards, EOF and no live owned processes. Next use the integrated release binary and the existing CEM
first-use harness to repeat the measurements below, retaining the original
assertions and distinguishing protocol time from menu-display time.

`E/cem-original-plan.json` records the recovered-root commands, JDK/home,
manifest/source expectations and UI recipe. Original eight checks mean four
corpus requests plus four K2 auxiliary checks; original eighteen combine the
main and incomplete manifests with eight auxiliary checks. Neither original
manifest asserts expression inference. The portable actual-buffer Rust case
now checks that inference without modifying the original buffers. Full CEM
editor validation passed against the integrated immutable dev binary
`serialization120/dev-shared5/bin/ktlsp.exe` (SHA-256
`6bbdcf7799236111cd7706efd09e356da9512d71ca6444391051b7afbfae6e56`),
with 430 recovered-input guards matched before launch. The main corpus retains
the four existing requests and four K2 checks, then adds one inference request;
the incomplete corpus retains its six requests and four K2 checks. Both runs
retain the coverage observations `missing_sources=3` and `failed=1`; selected
passing probes do not certify complete dependency-source coverage. That
checkpoint supplied no performance comparison; the measurements below use a
later integrated release. The integrated `native-integrated120-2` release
(SHA-256 `a099aeee9084d54e277a81db8c765bb67053ef6256c044e19f5dbf14437d8f44`)
subsequently passed all 26 original CEM semantic checks, with clean shutdown,
439 stable input guards and no live owned processes. Evidence is
`E/editor-original26-release1/summary.json`.

## Java calls to Kotlin constructor getters

The remaining source-coverage observations identify Patchy, TypeTools and the
KotlinForForge aggregate artifact; TypeTools also records an unsupported source
encoding failure. Missing source does not imply a missing API: representative
Patchy and TypeTools completion/navigation passed in both languages. All eight
nested KotlinForForge archives match separately selected dependency binaries.
The six-case external API probe isolated one real gap: Java `Pair.getFirst()`
completion and navigation were empty, while Kotlin `Pair.first` passed.

Kotlin primary-constructor properties now contribute JVM callable facts with
their real class owner, source definition and class type-parameter bindings.
Java completion, navigation and result chains consume the same candidate.
Annotated owners require an exact selected classfile getter and matching class
and method signatures; unknown property annotations, bounded parameters,
nested owners and value-class lowering remain unsupported in this slice.
The source-cache version is v35, rejecting pre-getter v34 caches.

Actual stdlib 2.4.0 Pair bytes and source, altered witness negatives, value-class
compiler controls and Kotlin getter-name controls cover this boundary. Core
tests pass 139 cases; the existing implicit-receiver suite passes 70, including
Java generic/String result chains, source edits and a Kotlin presentation
negative with compiled callables enabled. Evidence is `E/getter-summary.json`.
The unchanged six-case native API probe passed all 12 completion/navigation
assertions on their first responses with shared dev SHA-256
`07fc12c203d30cb5d391c4bca1ad06aa79de1067a858e7d6bd6fee4478292519`.
Shutdown completed with exit 0, EOF and no live or forcibly terminated owned
processes. All 36 semantic input guards remained stable. The original broad
guard result remains false because Gradle changed its generated filesystem
probe and file-hash lock; the extended-path audit classifies those two runtime
files separately and finds no unexpected input changes. Evidence is
`E/coverage/native-getter-fixed1/{summary,guard-audit,receipt}.json`.

Mixed Java/Kotlin constructor-property rename remains a UX gap: converting
`getValue` to `getRenamed` requires distinct edit spellings. Such mixed renames
return no edits instead of writing an invalid Java `renamed()` call. Kotlin-only
property rename remains supported and is checked by applying all returned edits.
This slice does not establish complete getter, rename or IDEA parity.

### Inherited constructor getters

An independent Kotlin 2.4.0/Javac 21 control confirms that Java
`Child extends KotlinBase<String>` inherits `getValue()` from
`KotlinBase<T>(val value: T)`, including the `getValue().length()` chain.
The corresponding Rust probe initially failed to offer the inherited getter.
Java callable collection now retains the original source property under its
derived JVM getter name during the existing single hierarchy traversal.
The existing visibility, unavailable-name and nearest-property override filters
run before projection. The callable keeps the declaring class identity and
exact source property definition; existing receiver bindings substitute the
full child receiver. Static facade lookup remains restricted to its own owner.
Serialized facts are unchanged, so this query change requires no cache bump.

The JVM focused suite passes 11 tests and the existing implicit-receiver suite
passes 71. The added regressions cover `getValue`/`isReady`, generic result
chains, private unsaved edits, a nearer unavailable property, empty-prefix
presentation and one source collection per child/base owner. Kotlin getter
presentation remains excluded by the existing negative case.

The existing corpus harness passes both native cases, all four
completion/navigation assertions on the first response, using shared dev
SHA-256 `902417c9e2abc7c3c6d2c9a0fce1410dd245110ccf0c4e291be72316a1c1c6b2`.
It verifies navigation to the exact Kotlin property and JDK `String.length`
declarations. Operations complete, shutdown exits 0, all input guards remain
stable, EOF is observed and no owned processes remain or require termination.
The first launch failed before semantic requests because the external manifest
used raw-byte hashes instead of the harness's joined-line hash contract.
That failed receipt is retained; correcting the hash metadata changed neither
source bytes nor expected declarations. Evidence is
`E/getter-inherited-summary.json` and
`E/coverage/native-inherited-fixed{1,2}/receipt.json`.

### Typed class-body and overriding property getters

The next compiler control confirms ordinary public typed class-body `val`/`var`
getters, a custom getter, generic constructor `open val`, and constructor/body
`override val`. Kotlin 2.4.0 and Javac 21 both exit 0 for all six Java getter
and result-chain calls. The Rust probe initially fails on the open constructor
getter. The source producer now accepts these supported property headers into
the same `SourceProperty` facts, retaining existing access, annotation,
value-class, owner and generic-bound checks. A member extension property is
excluded because its JVM getter has a receiver parameter. No new callable
selection or hierarchy traversal is added. The coordinated parser/binary
cache version is `v36-source-getters-lazy-binary`.

The JVM focused suite passes 11 tests and the implicit-receiver suite passes
72, including exact nearest override definitions and result chains. An unsaved
nearest override with unknown getter metadata yields no inherited base target;
restoring the source restores its getter. Body-property negatives retain
`JvmField`, `JvmName`, private/internal access, unknown owner annotations,
value-class returns and unavailable inferred types as unsupported.

The existing native corpus route passes six cases and all 12 first-response
completion/navigation assertions on shared dev SHA-256
`fe0977b6eda74847bb3250c33c607850c260ca16e2391bcaa45fc7d7fafbedfd`.
It checks the exact Base, Child, Body and BodyChild property definitions.
All 21 immutable input guards remain stable; operations complete with native
exit 0, EOF and no live or forcibly terminated owned processes. Evidence is
`E/getter-body-summary.json` and
`E/coverage/native-body-fixed1/receipt.json`.

Properties without an explicit type remain an inference coverage gap.
That typed-getter control checks a `var` getter only; setter coverage follows
below. Mixed derived getter rename remains unsupported.

### Java setters for mutable Kotlin properties

Supported public typed constructor/body `var` declarations now contribute
one-parameter setter facts beside their getters, sharing the same source
property definition and declaring class identity. Setter parameters use the
full receiver's existing generic binding; setter results are `Unit` and JVM
`void`, independently of the property's value type. The classfile witness
checks the real public instance method, parameter Signature and return
descriptor. No serialized field is added; the parser cache version is
`v37-property-setters-lazy-binary`.

Independent Kotlin 2.4.0/Javac 21 controls accept `Box<String>.setValue(String)`,
the nearest override, a body setter, and the actual `setReady`, `set_foo`,
`set0` and `setIsfoo` spellings. Javac rejects an Int argument, a String result
from the void setter, and a private setter. The shared solver asserts String
`Applicable` and Int `Rejected`, rather than accepting an unknown type.
One known invalid-call target retains the existing Java navigation contract;
argument rejection does not erase the known method's identity. Public getters
remain available when setters are private. Unsaved `var` to `val` edits remove
the setter and restoring the declaration restores it. The actual generic Box
classfile and source are portable fixtures with descriptor, generic Signature
and altered access/return negative controls.

The first native run exposed a real SDK-scope gap: generic erasure passed, but
concrete String/Boolean/Int accessors were missing (3 matched, 14 mismatched).
The value-class guard treated stdlib common `expect` declarations as proof of
mangled accessors, even with JVM `actual` declarations present. A portable
reduction using their real headers reproduces the same failure. The guard now
respects the existing intrinsic identity and scalar ABI proof together;
custom value classes and same-spelling user types do not gain that proof.
Annotations, unsupported bounds and unresolved types remain guarded.

After that fix, the JVM focused suite passes 11 tests and the implicit-receiver
suite passes 73. The unchanged native manifest passes eight cases: all 16 RPCs
match on their first response and all 17 semantic checks pass, including the
private-setter negative with a useful same-phase control. The shared dev is
SHA-256 `b86293a6cb243af4d77dfc58819d53c435f9831aaa56f36e3362e82eabbce571`.
Operations complete, native exit is 0, all 21 immutable input guards are stable,
EOF is observed and no owned processes remain or require termination.
The failed native receipt remains preserved. Evidence is `E/setter-summary.json`
and `E/coverage/native-setter-fixed{1,2}/receipt.json`. These are functional
checks, not a performance comparison or complete Kotlin property coverage;
inferred body properties and mixed derived accessor rename remain gaps.

## Integrated release first readiness and Neovim completion

`S = C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda/cem-speed120`.
`S/acceptance-plan.json` records commands and cache conditions;
`S/performance-summary.json` and `S/sessions-receipt.json` record results.
The immutable release is `native-integrated120-4/bin/ktlsp.exe`, SHA-256
`18938e7be4e847ad687e4c1c64559d48a19beb80315f80c128f8b16522141b61`.
The existing harness, original eight assertions, recovered CEM source/config,
JDK21 and Gradle home are preserved. All 438 input guards match before/after.
The initial heavy-process inventory was empty and peer heavy jobs were held.
Cold uses an absent LSP cache; warm and UI reuse that same cache without seeds,
copies or relocation. Both corpus sessions pass all eight assertions; all four
post-readiness requests match on their first response. All three server
sessions exit naturally with code0, EOF, no live owned processes or forced
termination. One failed dependency and three missing sources remain reported.

| Measurement | Historical113 | Current120-4 |
| --- | ---: | ---: |
| Cold first useful getBreakSpeed completion, n=1 | 170.189s | 52.547s |
| Warm first useful completion, n=1 | 3.453s | 1.689s |
| Warm RPC p50 / p95, n=30 | 15.234 / 32.586ms | 10.421 / 16.547ms |
| Warm input-to-menu p50 / p95, n=30 | 49.980 / 68.388ms | 44.026 / 51.686ms |

First-use polling remains 500ms and retains early empty/mismatched responses;
startup n=1 supplies no startup p95. UI has one separate first-after-readiness
sample (RPC9.916ms, input-to-menu44.662ms) followed by 30 warm samples.
Quantiles use linear interpolation at `(n-1)*q`, matching113. All 31 selections
are accepted with one completion request each, the whole original buffer hash
is restored, and disk bytes remain unchanged. Menu timing ends at Neovim's
`popupmenu_show` handoff, not physical display.

Current cold Gradle dump takes27.756s. Binary preview spans28.314–58.283s,
source query28.315–65.576s, and dependency indexing ends78.362s. Useful
completion at52.547s therefore precedes both complete binary preview and source
query completion: partial facts are usable before final source enrichment.
These overlapping wall-clock spans are not CPU samples.

Historical113 cold included first source-query use in the recovered Gradle
home; the current mandated home is already warm. The cold difference cannot
be attributed solely to the binary. An old-binary/new-cache control in the
same current Gradle home is deferred to a separate quiet window. Current cold
and warm library inventories also differ (46208/36040 files); selected eight
checks do not establish whole-index semantic equality. Broader functional
evidence remains separate, and these samples establish neither a global
latency budget nor complete dependency coverage or IDEA parity.

The next measured scope is Create's binary resolver:1627 files take17.367s,
ending52.423s immediately before first useful completion. JDK parsing already
ends33.039s. The binary path eagerly wrote every rendered Java stub and then
walked/read that directory to parse the same texts. It now visits the existing
renderer output directly, retains all selected class/descriptor/owner facts,
and materializes only a requested goto stub. A separate binary marker checks
the original artifact identity before/after work; it does not establish source
closure. The shared cache version is v36, also covering the subsequent source
property-getter parser changes.

The existing actual-three-Create-class test first failed its new no-eager-stub
assertion, then passed local/module cold/warm completion, exact goto bytes,
Float inference and parameter completion. It also checks changed/deleted JAR
rejection, removal of stale materialized text, and recovery after reappearance.
Repeated materialization validates identity but preserves the already generated
file's mtime instead of rendering again. A malformed marker rejects cached facts
and removes its old materialized text. Initial goto still reads the archive and
walks its inventory before decoding the selected owner; this residual cost is
not measured separately.
The existing binary suite passes16 tests and JAR suite29. Evidence is
`S/lazy-{red,green1,binary-suite,jar-suite,repeat-marker-green}.log`. At this
focused checkpoint, release timing was pending; the comparison below follows
the native check. Without CPU samples, the relative cost of I/O,
class decoding and parsing remains unmeasured.

Shared immutable dev SHA-256
`fe0977b6eda74847bb3250c33c607850c260ca16e2391bcaa45fc7d7fafbedfd`
then passed the unchanged original CEM26 (main9/incomplete17), retaining K2 and
Float inlay checks. Both runs exit naturally with code0, EOF, no live/forcibly
terminated owned processes, and439 stable guards. Each fresh v36 LSP cache
contains101 binary markers and zero eagerly materialized binary Java files.
The two returned goto URIs resolve to real files; their UTF-16 ranges select
the expected names and their complete normalized contents match the pinned
target hashes. These post-index goto targets use source markers, so this is
not a native binary-only goto proof. The portable actual-class test supplies
the binary-specific materialization coverage above. Evidence is
`S/editor-original26-lazy-dev1/{summary,receipt}.json`. This dev run supplies
functional evidence only; optimized-release measurements follow.

### Release120-6 comparison

`T = C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda/cem-speed120-6`.
The immutable optimized release is `native-integrated120-6/bin/ktlsp.exe`,
SHA-256 `2a02365ab3eb974941f21f9d73f9e675cf92e4dc9eb8bf3129d1e7eba381e158`.
The existing runner uses the same recovered CEM/JDK21/ordinary warm Gradle
home, original eight assertions and500ms first-use polling. Cold starts with
an absent LSP cache; warm/UI reuse exactly that cache, with no relocation or
seed. Start process inventory contains zero heavy processes and peers held
heavy work. All439 guards match before/after. Both corpus sessions pass8/8
with first-response matching; all31 UI selections are accepted, one request
each, with whole-buffer restoration and unchanged disk bytes. All three
servers exit naturally with code0, EOF, no live or forcibly terminated owned
processes. Failed1/missing sources3 remain coverage gaps.

| Measurement | Release120-4 | Release120-6 | Prospective target / observation |
| --- | ---: | ---: | --- |
| Cold first useful completion, n=1 | 52.547s | 35.969s | <=30s: missed |
| Full dependency-index span ends, n=1 | 78.362s | 56.696s | Reported separately |
| Warm first useful completion, n=1 | 1.689s | 1.700s | <=3s: within target in this observation |
| Warm RPC p95, n=30 | 16.547ms | 17.046ms | <=50ms: within target in this sample |
| Input-to-menu p95, n=30 | 51.686ms | 51.228ms | <=100ms: within target in this sample |

Current warm RPC p50/max are10.063/21.133ms; input-to-menu p50/max are
43.158/54.600ms. The separate first-after-readiness UI sample is RPC9.924ms,
input-to-menu43.822ms. Quantiles retain linear interpolation at `(n-1)*q`;
menu timing ends at Neovim's notification handoff, not physical display.

Cold first useful completion precedes the dependency-index span's end by
20.727s. Create binary resolution still covers1627 files and takes1.284s,
versus17.367s before; its end35.559s precedes the useful response35.969s.
The full binary-preview phase takes12.541s, with Gradle dump29.279s and
overlapping source query16.649s. These are wall-clock spans, not CPU samples.
The observed cold reduction is31.55%, but this compares whole releases and
does not isolate one patch or external Gradle state. Startup n=1 supplies no
startup p95 or reproducible attainment, and the cold target remains unmet.
Selected checks do not prove whole-index equivalence or IDEA parity. This
CEM run does not measure the separate five-project target.

Targets and conditions in `docs/fork-performance-budgets.md` were recorded
before measurement. Root updated only its closing observation paragraph
with a known Retrofit result after greenlight, before the actual guard
capture (06:13:08.357UTC versus06:13:29.262UTC). Both sessions retain the same
updated document hash. Reconstructing the prior closing paragraph reproduces
the prepared hash exactly, proving the target table and conditions unchanged.
Evidence: `T/{acceptance-plan,sessions-receipt,performance-summary,budget-guard-audit}.json`;
raw first-use and UI observations remain in `T/{cold,warm-ui}/artifacts/`.
