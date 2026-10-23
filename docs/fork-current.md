# Current fork state

Updated 2026-10-23 (Asia/Tokyo). Goal active. General IDEA parity and language-wide
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

`native-integrated120-30/main-ready.json`: workspace **1808 passed, 0 failed,
11 ignored, 42 targets**; release build passed. All577 guarded inputs remained
stable. Both owned jobs reached EOF with zero live processes and zero forced
cleanup. Workspace took166.063s and release build124.390s on this run; these are
observations, not editing-latency or isolated build-speed comparisons.

Immutable binary SHA256:
`824df805a26d264ba770665133a7e76ad2e8a56989f2be3eb7d3fb2ab2b2cf73`.
Its path is `native-integrated120-30/bin/ktlsp.exe` under RUN.

This binary includes the local-file archive-cache fix and exact declaration API.
The subsequent Project-component watch-baseline fix is not covered by this binary.

The next31 workspace run is retained as1807 passed/1 failed/11 ignored across42
targets (`native-integrated120-31/workspace-summary.json`). The existing
`watched_file_bursts_coalesce_and_active_refreshes_remain_singleflight` test did
not observe initial fake-Gradle startup within its12-second assertion. Its cause
is under focused investigation;31 did not build a release binary. Do not hide
this by enlarging its timeout or by reporting only successful target summaries.

## Current fixes and open observations

| Item | Actual evidence | Status / next action |
|---|---|---|
| Named Function0/1 trailing lambdas after named arguments | Focused semantic test passes; unchanged actual Neovim10 probes improve from2/10 on28 to10/10 on29, including5 exact source navigation targets. All first responses, stable guards and clean ownership. `named-function140-focus1/receipt.json`, `named-function140-release29-green1/receipt.json`. | Fixed for closed ordinary Function0/1, explicit/implicit parameters, Unit coercion and checked non-Unit results. Method-dependent callback types, receiver lambdas and multiple parameters remain conservative Unknown. |
| Unsaved Java dependency producer used by a Kotlin consumer | Actual compiler regression counter→retracted→restored passes with fresh private Kotlin+Java producer classes replacing exactly one selected JAR slot; incomplete/foreign-owner/processor/source/ambiguous-slot/SDK/task-key guards pass. Actual original Moshi shared16 also passes on29 with a correlated current-version clean compiler certificate. `moshi-cross140-green1/receipt.json`, `moshi-focused140-shared29/semantic-handoff.json`. | Fixed for the tested single producer. Multiple producers, dirty dependency Kotlin and unsupported compilation roles remain conservative. Generic whole-project producer-closure flags stay false. |
| Unchanged local JAR CHANGED notification | Gradle29 exposes missing baselines for58 selected Project-component paths and6 tracked fixture JARs. Separate meaningful REDs reproduce both. Selected File/Project origins plus Git-cached-only archive baselines now pass33 watch controls, preserving changed/new/untracked/unreadable cases. Only38 tracked JARs/124330 bytes are hashed in this actual project. `project-watch140-final.json`, `tracked-watch140-final.json`. | Included in the31 source under integration. Actual original17 replay still pending; no generic ignore of JARs under the project root. |
| Duplicate index bucket scans | Strengthened replacement test and c18 control pass. Instrumented actual method reduces retain-predicate visits24960→576, preserving the other file's64 overloads. `index-bucket139-count1/receipt.json`. | Included in29. Operation count only; no claimed end-to-end multiplier or attribution of Gradle's aggregate indexing time. |
| Same-size/same-mtime JAR replacement | Genuine warm-cache RED returns old members. New content-partitioned disk cache and atomic retirement of completed memory origins pass actual watch→model/index reload, Java/Kotlin static/instance completion and open-buffer preservation. Focused regression+controls52 executions pass, with overlapping controls. `archive-cache140-final.json`. | Included in30; final lifecycle editor replay pending. No sibling source archive in this reproduction. Unchanged adjacent sources can still describe outdated binary API; Project-artifact same-metadata semantic cache freshness is also unproved. |
| Exact source declaration→extension TypeId | `ReadSession::declaration_type` uses exact scoped identity and tracked lifetime/namespace/model inputs; all45 core extension tests pass. External provider uses this API and `TypeId::package()`; own-lock4 tests/build and unchanged90 editor requests/144 checks pass. `external-public140-30/{cargo,editor}-receipt.json`. | Committed/pushed in83d9e16 and7e53664. Nested owners and exact package configuration are checked in the existing unit test; no editor expectation/count changed. Members return Unknown rather than their owner/value type. |

## Actual project and lifecycle acceptance

These are finite selected workflows, not whole-project absence-of-errors certificates.
No project source is rewritten for tests: unsaved edits or disposable private copies
are used, with original contents and Git metadata guarded.

| Requirement / project | Latest retained result | Remaining verification |
|---|---|---|
| CEM excluding Mixin | Selected38 assertions /21 first responses on25-2, including original26, F-bounded getter8 and TypeTools4. | Final immutable binary replay. |
| Additional five projects | Original50 first responses pass on25-2: Gradle9, Moshi9, OkHttp10, Retrofit12, Spring10. | Final immutable replay; original50 contains no diagnostic expectations and does not certify all syntax. |
| Moshi mixed shared edits | Original atomic6 passes on28. Shared16 improves from15 matched/1 unmeasured on28 to16/16 first responses on29, including current Kotlin1.8.21 counter diagnostics. Original/copied sources and JAR unchanged; original missing producer JAR stays absent. `moshi-focused140-shared29/semantic-handoff.json`. | Tested workflow achieved. Editor API retraction completion/goto passes; compiler retraction diagnostics are separately established by the native lifecycle case. |
| Spring mixed shared edits | Original16/16 first responses pass on28, including current-version complete compiler pass certifying empty document diagnostics.10861 original tracked inputs unchanged. `spring-shared138-final.json`. | Whole-project producer closure remains false; do not broaden the document-level claim. |
| Retrofit mixed shared edits | Portable original22 first responses pass on27; exact invalid-diagnostic range separately audited against the same payload. Applied rename inspected. `retrofit-shared137-portable27/final-summary.json`. | Final source changes must retain the existing expectations. |
| Gradle mixed shared edits |29 closes13 matched/0 mismatch/4 unmeasured with stable19 guards and clean ownership; original25569-byte manifest union matches using extended paths. Renamed goto now matches, retracted-member completion is unanswered and3 canonical diagnostics never start. `gradle-shared134-handoff29.json`. | Await31 original17 after actual watch-cause repairs; do not count an empty fast diagnostic push as canonical certification. |
| Libraries added/replaced by real Gradle builds | Versioned90 and same-path47 checks pass on25-2, with actual builds and changed configuration/JAR proof. The earlier46/47 run remains historical unmeasured evidence. | Final immutable lifecycle replay. |
| Compiler plugins / generated sources | KAPT/KSP114 passes on26; actual compiler controls have expected exits0,0,1(disabled),0(restored), including60 RPCs and generated-current certification. | Final immutable replay. No arbitrary compiler-plugin compatibility claim; producer closure flag remains false where recorded. |
| External shared-semantic extension | Current exact-declaration provider: independently built own-lock crate4 tests/build; actual Neovim90 first responses/144 assertions pass, with stable guards and clean ownership. `external-public140-30/editor-receipt.json`. Enabled/disabled/config/edit consistency includes completion, diagnostics, goto, references and applied rename. | Tested shared boundary achieved. Production Mixin is a separate project; IPC remains unimplemented. |
| IDEA comparison | Recorded selected semantic comparisons include original50 composed observations plus exact owner/signature controls; operational shutdown failures are retained separately. | General IDEA parity and a clean single same-state full comparison remain unproved. |
| Warm responsiveness | Earlier release warm30 RPC p95 18.577ms; input-to-menu p95 52.326ms; warm startup n5 p95 1062.820ms. | Latest distribution unmeasured; user currently prioritizes Unknown coverage. These are not pixel-display timings. |

## Extension boundary and mc-dev handoff

The current contract is compile-time Rust composition through
`Backend::with_analysis_providers`, with shared semantic contributions and tracked
invalidation. A shared LSP connection or appended completion items alone is insufficient.
There is no stable dynamic ABI or implemented IPC transport. A future versioned
stdio boundary must preserve snapshot identities, tracked reads, cancellation and
stale-result rejection, without waiting for IPC while holding Workspace locks.

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
completion. Core fixes and six-project validation remain first. No IPC code has
been implemented or verified yet.
Provider-specific structured diagnostic codes are a recorded nonblocking follow-up;
do not introduce a broad diagnostic enum/transport refactor for the current migration.
Full MixinExtras/Lapis, generated/multiple-target and complex generic/value-class
semantics are not certified by the test extension.

## Next actions and execution rules

1. Resolve the31 startup-test failure, integrate a coherent next source snapshot,
   and finish the unchanged Gradle17 editor observation. Preserve failed or
   unmeasured first responses; elapsed deadlines never count as semantic success.
2. Retain focused cache/watch and external consumer evidence while verifying integration.
3. Build one coherent final source snapshot, then replay existing CEM+five-project
   and selected plugin/dependency lifecycle cases. Reuse existing controllers and
   expectations; do not create another test framework or repeat unaffected suites.
4. Update this short record, selectively commit/push verified changes, and report
   practical coverage and remaining limits. Do not derive grammar coverage from counts.
5. Add the general asynchronous IPC spike as a separate reviewed slice; keep MC rules
   and full domain migration in mc-dev, with no communication waits under Workspace locks.

Native delegated work uses gpt-6.1-sol/high, at most5 agents, no new worktrees.
One Cargo owner and a global Rust write freeze cover each native build. Immutable
editor runs can overlap later source work. Cache directories are private; stdout
stays JSON-RPC only. Keep raw logs on disk and read bounded summaries or exact errors.
Preserve user AGENTS.md, unrelated untracked files and historical failure receipts.
