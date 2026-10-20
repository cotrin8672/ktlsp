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
| Full release CEM cold/warm/UI and broad workspace checks | Pending integrated validation | No current speed or IDEA-parity claim from these reductions |
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
first-use harness to measure whether useful completion now precedes source
enrichment, retaining the original assertions and distinguishing protocol time
from menu-display time. This slice supplies no new performance budget or sample.

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
passing probes do not certify complete dependency-source coverage. Performance
awaits an integrated release build.
