# Current fork state

Updated 2026-10-24 (Asia/Tokyo). Goal active. General IDEA parity and language-wide
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

`native-integrated120-35b/main-ready.json`: workspace **1821 passed, 0 failed,
11 ignored, 42 targets**; release build passed. All583 guarded inputs remained
stable. Both owned jobs reached EOF with zero live processes and zero forced
cleanup. Raw timings remain in the receipts; these checks do not measure editing
latency or establish an isolated build-speed comparison.

Immutable binary SHA256:
`7e71f6f44c12c3a80a0ff60363d1a6ee996dfb2e4b3e010cf3fc12d81c68fcd8`.
Its path is `native-integrated120-35b/bin/ktlsp.exe` under RUN.

This binary includes the earlier local-file archive-cache and exact-declaration
fixes, archive input staging before initial watch registration, the asynchronous
semantic IPC host with exact declaration-member facts, bounded compiler-result
tracing, generic callback input/result inference, and a generic IPC publication
trace. Actual35b callback and extension checks pass below. Gradle's original17
workflow is still running against immutable34; integration is not proof that all
remaining editor acceptance cases have passed.

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

## Current fixes and open observations

| Item | Actual evidence | Status / next action |
|---|---|---|
| Named Function0/1 trailing lambdas after named arguments | Focused semantic test passes; unchanged actual Neovim10 probes improve from2/10 on28 to10/10 on29, including5 exact source navigation targets. All first responses, stable guards and clean ownership. `named-function140-focus1/receipt.json`, `named-function140-release29-green1/receipt.json`. | Fixed for closed ordinary Function0/1, explicit/implicit parameters, Unit coercion and checked non-Unit results. Method-dependent callback types, receiver lambdas and multiple parameters remain conservative Unknown. |
| Named generic callback context and call result | A compiler-valid ordinary callback caused a stack failure before completion on33; a result-chain variant returned no useful completion. Shared context inference now avoids re-entering the body and binds method variables from ordinary named arguments. New/old focused regressions and workspace pass. Actual34 preserves both original sources:2 useful first completions and2 exact source gotos, all clean with stable guards (`completion-gap141-final.json`). | Fixed for the tested ordinary-argument-bound callback and result chain. Negative annotation/result/name controls remain conservative. This does not certify every higher-order generic or receiver-lambda form. |
| Higher-order generic callback result | Compiler-valid `transform<A,B>(value:A, block:(A)->B)` gives0/2 useful first completions on34. Context now requires known callback inputs, while the checked body result participates in the existing generic solver. Actual35b returns both `Candidate.convert` and result `Other.usefulOther` on the original first responses, with compiler proof, unchanged inputs and clean ownership (`completion-gap142-final-green.json`). | Fixed for the tested Function0/1 slice, retaining Unit coercion and negative name/annotation/result controls. Unbound results with additional upper bounds and unsupported receiver/multiple-parameter forms remain conservative Unknown. |
| Unsaved Java dependency producer used by a Kotlin consumer | Actual compiler regression counter→retracted→restored passes with fresh private Kotlin+Java producer classes replacing exactly one selected JAR slot; incomplete/foreign-owner/processor/source/ambiguous-slot/SDK/task-key guards pass. Actual original Moshi shared16 also passes on29 with a correlated current-version clean compiler certificate. `moshi-cross140-green1/receipt.json`, `moshi-focused140-shared29/semantic-handoff.json`. | Fixed for the tested single producer. Multiple producers, dirty dependency Kotlin and unsupported compilation roles remain conservative. Generic whole-project producer-closure flags stay false. |
| Unchanged local JAR CHANGED notification | Gradle29 exposes missing baselines for58 selected Project-component paths and6 tracked fixture JARs. Separate meaningful REDs reproduce both. Selected File/Project origins plus Git-cached-only archive baselines now pass33 watch controls, preserving changed/new/untracked/unreadable cases. Only38 tracked JARs/124330 bytes are hashed in this actual project. `project-watch140-final.json`, `tracked-watch140-final.json`. | Included in the31 source under integration. Actual original17 replay still pending; no generic ignore of JARs under the project root. |
| Duplicate index bucket scans | Strengthened replacement test and c18 control pass. Instrumented actual method reduces retain-predicate visits24960→576, preserving the other file's64 overloads. `index-bucket139-count1/receipt.json`. | Included in29. Operation count only; no claimed end-to-end multiplier or attribution of Gradle's aggregate indexing time. |
| Same-size/same-mtime JAR replacement | Genuine warm-cache RED returns old members. New content-partitioned disk cache and atomic retirement of completed memory origins pass actual watch→model/index reload, Java/Kotlin static/instance completion and open-buffer preservation. Focused regression+controls52 executions pass, with overlapping controls. `archive-cache140-final.json`. | Included in30; final lifecycle editor replay pending. No sibling source archive in this reproduction. Unchanged adjacent sources can still describe outdated binary API; Project-artifact same-metadata semantic cache freshness is also unproved. |
| Exact source declaration→extension TypeId | `ReadSession::declaration_type` uses exact scoped identity and tracked lifetime/namespace/model inputs; all45 core extension tests pass. External provider uses this API and `TypeId::package()`; own-lock4 tests/build and unchanged90 editor requests/144 checks pass. `external-public140-30/{cargo,editor}-receipt.json`. | Committed/pushed in83d9e16 and7e53664. Nested owners and exact package configuration are checked in the existing unit test; no editor expectation/count changed. Members return Unknown rather than their owner/value type. |
| Exact authored declaration→extension member | `declaration_member`/wire `declarationMember` uses exact definition and a unique owner, reuses normalized member facts and tracks source/model/namespace reads. Focused core4/wire2 pass. The child uses the mandatory query instead of name matching; actual34 IPC26/linked14 and equal completion/goto/references/applied rename pass (`member141-final-receipt.json`). | Available in34. Foreign/stale tokens are rejected; ambiguous owners, incomplete facts and unresolved multiple JVM projections remain Unknown. |
| Generic IPC publication observation | `extension.ipc.accepted` is emitted only after a valid current commit reaches the shared Index. It records provider/owner/instance/evaluation/publication revision/source UTF-8 SHA256; tracing off adds no digest work. Focused host5 and actual35b IPC26/linked14 pass; fresh/restart barriers precede the first semantic RPC, and completion/goto/references/applied3-buffer rename agree (`publication142-final.json`). | Verified without framework diagnostic-marker readiness or semantic retries. This debug event is not persistent readiness or producer closure; later edits, disable and child exits retract contributions. |

## Actual project and lifecycle acceptance

These are finite selected workflows, not whole-project absence-of-errors certificates.
No project source is rewritten for tests: unsaved edits or disposable private copies
are used, with original contents and Git metadata guarded.

| Requirement / project | Latest retained result | Remaining verification |
|---|---|---|
| CEM excluding Mixin | Actual32 closes36 matched/0 mismatch/2 unmeasured out of38: normal7/9, incomplete17/17, getter8/8 and TypeTools4/4. All owned runs reach EOF with stable guards, no live children or forced cleanup. Both unmeasured checks require canonical Java diagnostics; raw javac output contains the intended error, but canonical publication/return is unproved. `cem-remaining140-release32.json` links the separate receipts, including the failed normal stage. | Investigate final diagnostic admission with a diagnostic-only reproduction. Do not call this38/38 or a whole-project certificate. |
| CEM canonical diagnostic follow-up | Release33 now passes both original canonical checks with current-v3 certification and both buffer-preservation controls. Exact source SHA is audited;447 guards stable, EOF,0 live/forced cleanup (`cem-startup120/editor-original26-diagnostics140-release33-warm1/acceptance-release33.json`). Earlier debug follow-up also passes and remains retained. | The two previously unavailable32 checks are verified on33. No diagnostic semantic fix was required; timing evidence supports prior work exceeding the observation window, while its exact old finish gate was unobserved. This is not a whole-project or latency certificate. |
| Additional five projects | Actual32 verifies41/41 first responses: Moshi9, OkHttp10, Retrofit12 and Spring10. All8 stages have stable guards, clean LSP shutdown, EOF and zero live/forced cleanup (`final-corpus140-release32/independent41-summary.json`). The exact missing SDK34 prerequisite was restored from verified official archive bytes before Retrofit ran. | Gradle9 is explicitly not run after the startup-churn abort. Verify after the provisional-baseline fix; original50 contains no diagnostic expectations and does not certify all syntax. |
| Moshi mixed shared edits | Original atomic6 passes on28. Shared16 improves from15 matched/1 unmeasured on28 to16/16 first responses on29, including current Kotlin1.8.21 counter diagnostics. Original/copied sources and JAR unchanged; original missing producer JAR stays absent. `moshi-focused140-shared29/semantic-handoff.json`. | Tested workflow achieved. Editor API retraction completion/goto passes; compiler retraction diagnostics are separately established by the native lifecycle case. |
| Spring mixed shared edits | Original16/16 first responses pass on28, including current-version complete compiler pass certifying empty document diagnostics.10861 original tracked inputs unchanged. `spring-shared138-final.json`. | Whole-project producer closure remains false; do not broaden the document-level claim. |
| Retrofit mixed shared edits | Portable original22 first responses pass on27; exact invalid-diagnostic range separately audited against the same payload. Applied rename inspected. `retrofit-shared137-portable27/final-summary.json`. | Final source changes must retain the existing expectations. |
| Gradle mixed shared edits |29 closes13 matched/0 mismatch/4 unmeasured. Actual32 reaches none of17 probes because early selected-JAR notifications repeatedly invalidate initial import. The pre-registration baseline fix is included in34. After genuine missing-input/model restoration, the original17 replay accepts GEN1 and reports no forced JAR refresh; semantic first responses are arriving. Source CHANGED notifications then trigger further full refreshes and cancel a later diagnostic ticket. | Current34 run remains in progress, with no final17 verdict. Establish whether these source events changed disk bytes and fix only the reproduced cause. Original9 follows serially; retain earlier abort and unmeasured responses. |
| Libraries added/replaced by real Gradle builds | Actual33 versioned90/61 first RPCs/4 builds and same-path47/23 first RPCs/2 builds both pass. New API, old API withdrawal, exact source navigation, current diagnostics and unsaved-buffer preservation are checked; all8 guards stable, clean shutdown/EOF/0 live/forced cleanup (`dependency-lifecycle141-final.json`). | Tested lifecycle workflows achieved. Missing dependency and source archives were restored by genuine Gradle resolution; the original offline acceptance stayed unchanged. Failed setup/readiness runs are retained separately, with no latency claim. |
| Compiler plugins / generated sources | Historical32 has109/114 pass and correct0,0,1(disabled),0(restored) builds. The5 unavailable/mismatched phase1 requirements now pass within original focused8 on33: Java/Kotlin each one diagnostic response, exact3 obsolete calls per language, current v4 SHA/result identity,2 actual builds/RuntimeCheck12 each,32 stable guards and clean EOF/0 live/forced cleanup (`processor120/processor-phase1-release33-resume141b/semantic-adjudication141.json`). | Focused semantic gap closed. The original cold indexing120s check still fails, so that run is34/35 and exit1; do not call the full114 rerun or all35 successful. No semantic retry or deadline increase; producer closure remains false. |
| External shared-semantic extension | Embedded exact-declaration provider: own-lock4 tests/build and Neovim90 first responses/144 assertions pass (`external-public140-30/editor-receipt.json`). New actual child IPC26 checks/12 extra RPCs and linked14 checks/6 RPCs both close cleanly with stable guards, EOF and no live/forced cleanup. Same-input completion edits, goto, references and applied3-buffer rename match (`ipc140-editor-2/receipt.json`, `ipc140-editor-linked-1/receipt.json`, `ipc140-linked-equivalence.json`). | Single Java shadow-method IPC spike achieved, including cancellation/stale rejection, idle-crash index retraction, restart and disable. Kotlin/fields/unopened-dependency wire acceptance and full MC policy remain separate. The first fixture-restoration failure is retained in `ipc140-editor-1`. |
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

1. Finish actual Gradle17 plus the original9 after genuine missing-input restoration;
   verify the archive baseline fix and investigate source-triggered full refreshes.
   Preserve failed or
   unmeasured first responses; elapsed deadlines never count as semantic success.
2. Retain focused cache/watch and external consumer evidence while verifying integration.
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
