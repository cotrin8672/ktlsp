# Current fork state

Updated 2026-10-25 (Asia/Tokyo). Goal active. General IDEA parity and language-wide
correctness coverage are unmeasured; passing selected probes does not establish either.
User priority is valid-code Unknown/missing completion and false diagnostics, followed
by editing UX. Warm speed is accepted provisionally; cold startup is lower priority.

Evidence paths below are relative to RUN:
`C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda`.
The complete previous record is preserved in
[history through slice139](fork-history-through-139.md). Older acceptance matrices
are historical observations, not a list of current regressions. Performance targets
and comparison rules remain in [performance budgets](fork-performance-budgets.md).

## Latest integrated source and binary

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

## Focused follow-up, not yet integrated

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

## Actual project and lifecycle acceptance

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

1. Qualify the Gradle counter case under the actual producer's language level and
   processors, then implement the validated producer route. Preserve the original
   unmeasured response; elapsed deadlines never count as semantic success.
2. Retain the verified six-origin POM policy/byte audit and independent String-only
   eligibility evidence. Retain source-charset Unknown where metadata is missing.
3. Build one coherent final source snapshot, then replay existing CEM+five-project
   and selected plugin/dependency lifecycle cases. Reuse existing controllers and
   expectations; do not create another test framework or repeat unaffected suites.
4. Update this short record, selectively commit/push verified changes, and report
   practical coverage and remaining limits. Do not derive grammar coverage from counts.
5. Keep the verified asynchronous IPC boundary usable by the independent mc-dev
   consumer; later fact capabilities remain scoped additions, with MC policy in mc-dev
   and no communication waits under Workspace locks.

The human permits ongoing API-contract, implementation-commit and verification-result
sharing with the same mc-dev task within this goal. That scoped technical coordination
does not require another permission request for each message.

Native delegated work uses gpt-6.1-sol/high, at most5 agents, no new worktrees.
One Cargo owner and a global Rust write freeze cover each native build. Immutable
editor runs can overlap later source work. Cache directories are private; stdout
stays JSON-RPC only. Keep raw logs on disk and read bounded summaries or exact errors.
Preserve user AGENTS.md, unrelated untracked files and historical failure receipts.
