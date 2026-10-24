# Semantic extension IPC v1 — first implemented slice

Status: the stock Backend loader, child worker, scoped fact/requirement batches and shared
commit path are implemented. Windows debug builds passed the real Neovim single-Java-method
slice below, including a separately loaded child and a linked-provider comparison. This
does not establish full Mixin, Kotlin, field or dependency lifecycle parity. Production
Minecraft policy stays in mc-dev; the fixture alone contains the shadow-style test rule.
The host keeps one editor-facing LSP connection. The wire is experimental and should be
pinned with the host revision; no stable native ABI is promised.

## Boundary

Use one long-lived, explicitly registered stdio JSON-RPC child per provider/workspace. This
allows later installation and independent updates without a Rust dynamic-library ABI.
The cost is asynchronous publication, owned wire facts and generation validation. An
existing public Rust type is not a stable IPC or native ABI commitment.

The host owns project/classpath selection, Java/Kotlin unsaved input, normalized facts,
identity issuance, read tracking, member solving and publication. The child owns framework
rules and proposes requirements, source sites and diagnostics. Completion, navigation,
references and rename consume the same accepted contribution sets as linked providers.
The child does not create an independent project model or return feature-specific LSP
answers through this semantic protocol.

Executable path and argv are explicit user registration, separate from provider
`enabled`/`configuration`. Spawn without a shell; opening a repository must not execute
repository-selected programs automatically. Initialization options and
`workspace/didChangeConfiguration` accept an explicit registration:

```json
{"analysisProviders":{"example.semantic":{"command":["/absolute/path/to/provider"],"enabled":true,"configuration":{}}}}
```

`command` is a nonempty executable/argv array, not shell text. Existing linked-provider
settings still use `enabled`/`configuration` without `command`. stdout carries framed JSON;
stderr is drained without retaining output. Explicit re-enable/configuration change after
a failure creates a new worker; there is no automatic restart loop. Restart creates a new
provider instance and invalidates its handles. stdio isolation is not an OS sandbox.

## Minimum wire contract

JSON-RPC 2.0 uses UTF-8 bodies and Content-Length framing, independently versioned from
Rust crate revisions. `initialize` exchanges protocol major/minor, provider identity,
instance and required capabilities. Reject an incompatible major or missing required
capability for that provider only; optional additions need negotiated support. No stable
native ABI is implied. Method names below describe the current v1 vocabulary.

| Direction / operation | Minimum meaning |
| --- | --- |
| Host → child `initialize` | Major 1 / semantic-v1 capability agreement. Configuration accompanies each evaluation. Host shutdown closes and reaps the owned child; a graceful child shutdown/exit exchange is not implemented yet. |
| Host → child `analysis/evaluate` | Evaluation token, owner identity and current normalized source/declaration/annotation facts. The response completes this evaluation with proposed sites and diagnostics. |
| Child → host `analysis/readFacts` | A bounded batch of scoped type/declaration/member/signature reads for that evaluation. Host performs the existing ReadSession queries and returns owned facts. |
| Child → host `analysis/require` | A bounded batch of MemberRequirement proposals. Host resolves them, returning opaque requirement tokens, candidates and binding knowledge without publishing. |
| Host → child `$/cancelRequest` | Cancel superseded work; a late response is invalid regardless of child cooperation. |

Every evaluation-related message carries the host evaluation token. All identities are
host-issued opaque strings: declaration, type, member, signature/type-reference and
requirement tokens refer to host-owned values in an evaluation/instance registry. Never
serialize private TypeId/DeclarationId fields, accept a child-supplied declaration identity,
or reconstruct an identity from a qualified name. Names are query input, not authority.
Reject cross-workspace, foreign-instance, stale or wrongly typed tokens.

`analysis/readFacts` accepts `resolveType`, `resolveTypeRef`, `declarationType`,
`declarationMember` and `declaredMembers` operations. For example,
`{"op":"declarationMember","declaration":"<host declaration token>"}` returns a
Fact containing the exact authored MemberCandidate, with opaque member/signature tokens
and the existing normalized kind/descriptor/dispatch/visibility/synthetic fields.
The host matches the exact indexed definition and a unique containing type, then reuses
`declared_members`; it does not select a same-name overload. Non-member declarations,
missing facts, incomplete scope and ambiguous owners remain Unknown. Foreign/stale
identities are rejected using the existing contract checks. Source, model and namespace
reads participate in the same resume/commit validation as other facts. This additive query
is newer than snapshot `58ce9c41` and is verified in release34: focused core4/wire2
checks, the workspace suite, and the independently built child on the actual host.
The child now requires this query instead of matching a same-name member.
`member141-final-receipt.json` records clean IPC26/linked14 checks and equal completion,
goto, references and applied rename outputs.
An authored Kotlin property with accessor projections, or an unsupported `@JvmOverloads`
lowering, does not prove a single JVM member; unresolved signature/modifier facts keep the
whole correspondence Unknown. This query does not enumerate generated JVM projections.

Source facts preserve owner/annotation/declaration ranges, normalized annotation arguments,
member kind and known signature/dispatch/visibility/synthetic facts for both languages.
Class-valued annotation/type-reference tokens are resolved by the host in the owner scope.
JVM descriptors are host-validated facts, not a license to erase unresolved or generic
types to Object. Facts use distinct Found, Unknown-with-reasons and DefinitelyAbsent
variants. Coverage/inventory reads are explicit; an incomplete list cannot prove absence.

`analysis/require` expresses the current MemberRequirement fields: target knowledge,
optional authored declaration, exact/prefix names, kind, signature and dispatch knowledge,
and name link. The host uses `ReadSession::require` and `resolution`; the child cannot
substitute its own successful binding. Final sites reference those requirement tokens and
carry full/editable UTF-8 byte ranges and rename subject. The host uses `bind_site` to
validate ranges, spelling, projection and overlaps. Diagnostics use the core severity,
range, code and message shape and pass `diagnostic` validation. Successful evaluation
atomically replaces one provider/owner set; partial replies never become visible.

## Smallest bridge to the current synchronous core

Today `AnalysisExtensions::evaluate` calls synchronous `AnalysisProvider::analyze`, then
`ReadSession::finish` and `ExtensionStore::commit`/`fail`. Both ReadSession and ScopedIndex
borrow the live store/index. They cannot be retained across child I/O, and waiting inside
the existing analyze call would hold Workspace access needed by ordinary requests.

Reuse the owned `PendingContributions` already returned by finish. It already contains the
provider/owner/evaluation, accumulated read set and uncommitted contribution set. The
core change retains its session identity and adds a narrow resume operation:
validate with the existing `validate_pending` helper, check the supplied owner, and rebuild
a short-lived ReadSession over the current scoped index. Do not duplicate a weaker stale
check, clone the Index or move Workspace ownership to the child worker.

The host starts an evaluation under a short lock, materializes facts and finishes it into
owned pending state. On each read/require/final batch, it resumes, validates **all prior
reads and source/config/provider/model/namespace generations before reading anything**,
performs existing session operations, then finishes again. Communication holds only owned
facts/tokens/pending state: no borrowed session and no Workspace mutex. Final publication
uses the ordinary commit path, followed by the existing index contribution publication
and diagnostic-change scheduling.

The evaluation token represents an optimistic snapshot transaction, not an Index clone.
Every returned fact must still coexist at commit. Reads added in later batches are tracked
by the host, including negative type/inventory reads and missing source producers. If any
previously consumed input changes, abandon the whole evaluation rather than mixing its
old facts with a new batch. Project/dependency/generated-input acceptance must use the
same model/compiled generation invalidation as linked providers.

Existing source hydration discovers missing-source reads only through committed sets.
Staged remote reads must also feed that same producer frontier through a narrow pending
read accessor, without publishing staged semantic results. Source acceptance can invalidate
an active transaction; cancel/requeue its owner, not just the newly hydrated source file.
Keep this background wanted-source frontier separate from the completion-await predicate:
an ordinary request must never wait for a source solely because a remote staged read wants
it. Verify staged negative lookup/source arrival separately after the basic local barrier.

Keep linked providers on their existing synchronous path. Remote registrations use a
separate queued path in AnalysisExtensions; they must not call the transport from analyze.
The LSP scheduler takes owned work, communicates outside the lock, then re-enters only for
short validation/query/publication operations. Coalesce superseded owner work and bound
frames, fact batches, queue size, evaluation lifetime and restart attempts. Exact protection
values require implementation review; none are performance claims.

## Pending results and failure

Pending extension-dependent knowledge is Unknown, never DefinitelyAbsent. Do not turn an
unavailable extension result into a missing-target diagnostic, or suppress unrelated
ordinary diagnostics. No unvalidated site/binding becomes temporarily authoritative.
Normal completion must not await the child. The current `await_extension_sources` is the
existing dependency-source hydration mechanism; IPC readiness must not be added to it.
After accepted contributions arrive, republish affected diagnostics and let a new standard
completion request consume them. LSP has no general completion-refresh request; document
pending completions as incomplete where appropriate rather than inventing one.

Edits/config changes cancel affected evaluations and retract invalid sets immediately.
Timeout/failure removes the affected evaluation's contribution; crash/protocol failure
retracts all sets and handles for that provider instance. A delayed failure cannot retract
a newer successful result. Ordinary analysis and other providers continue. Shutdown must
close workers and reap the exact owned child outside Workspace locks. Reuse existing
owned-process cleanup instead of creating a competing process-management framework.

## Real host spike: observed results and remaining acceptance

Use the existing external-extension fixture/corpus harness with one target and one
shadow-style field/method whose JVM descriptor is known. Framework policy in this fixture
is test-only, not production MC logic in the host. Compare the child with the linked
provider using the same Java/Kotlin source and one frontend connection.

The verified sequence is deterministic, not a latency threshold:

1. A real loaded child reaches a barrier after reading the target. Record its paused
   evaluation token; do not release it yet.
2. Send an actual ordinary completion request for a known local API through Backend/the
   editor connection, with no dependency hydration needed for this assertion.
   Assert the useful normal result arrives **before** releasing the child.
3. Deliver an unsaved target edit through that same connection, then release the old
   child response. Assert its generation check rejects it and no stale site/diagnostic
   is published.
4. Allow a fresh evaluation and compare its descriptor-bearing completion/edit range,
   goto, references and applied whole-buffer rename with the linked result on the same
   unsaved input. Verify the IPC diagnostic oracle marker through normal LSP diagnostics
   separately; it is not a claim of complete Minecraft diagnostic-policy equivalence.

The child also exited while idle after publication: the target relation disappeared from
the actual editor's goto result, its diagnostic was withdrawn, ordinary completion still
worked, and explicit re-enable restored the binding. Disable withdrew the relation again.
Both three-buffer rename results were compared byte-for-byte after applying edits.

Evidence root `R` is
`C:/Users/gummy/.codex/ktlsp-runs/01a12550-7d56-71f2-8c34-8ab8c5fe5dda`.
These are captured working-tree source fingerprints and copied debug binaries, not a
claim that an uncommitted source tree has a release pin.

| Case | Observed evidence | Limit / next action |
| --- | --- | --- |
| Detached core transaction | `R/ipc140-core-1/receipt.json`: 2 tests passed, including preserved requirement token and source/owner/config/model/namespace/new-evaluation/foreign-owner rejection. | Does not establish editor coverage of each invalidation source. |
| IPC boundary/lifecycle guards | `R/ipc140-focused-2/receipt.json`: 4 passed; framing, foreign-token poisoning, staged-source frontier separated from completion await, late old failure after replacement success. | A later equivalent EOF/queue-overflow cleanup branch was compiled in build 2; no dedicated overflow test claim. |
| Real loaded child | `R/ipc140-editor-2/receipt.json`: 26 semantic checks, 12 extra-probe RPCs, no mismatches/unmeasured results; operations complete. | One Java target and one Java shadow-style method, known `(I)I` descriptor. Field and Kotlin wire/editor cases remain open. |
| Same-input linked control | `R/ipc140-editor-linked-1/receipt.json`: 14 checks / 6 extra-probe RPCs; `R/ipc140-linked-equivalence.json` matches completion projection, goto, references and all three renamed buffers. | Not the historical 90-request fixture suite or full MC diagnostic parity. |
| Process/input integrity | Both clean editor receipts: runtime input guards stable, EOF true, owned live count 0, forced residual cleanup 0. Build receipts and binary fingerprints are in `R/ipc140-host-build-2/ready.json`. | First editor attempt `R/ipc140-editor-1` retained its exit-1 cleanup failure: restored buffers still had modified flags. The hook now verifies original contents before resetting those flags. |

The tested host binary SHA-256 is
`baf9bd0250cae200a48d7dfc1e0ae2f36dc6e7b143105687f48259bf5508e317`.
The binary source guards include the untracked transport, host adapter and child source;
editor receipts separately guard the copied binaries, probe, manifest and disposable inputs.

Also cover disabled/config-changed behavior, reverse Java/Kotlin target/owner inputs,
static mismatch, incomplete/absent targets, negative lookup followed by source arrival,
dependency/generated-source invalidation, cancel/crash retraction and a fresh restart.
Cases beyond the table remain acceptance work. In particular, no real unopened dependency
or generated-source IPC editor lifecycle has been run. Late registration must retain honest
Unknown facts for unavailable closed-source inputs; source arrival is covered only by the
focused host guard. The current wire diagnostics accept the existing core diagnostic codes,
not the complete external MC code catalog. Timeout/protocol-mismatch rejection, Kotlin and
field editor coverage, and release timing/RSS budgets remain unmeasured. A standalone core
or framing test cannot substitute for those editor checks.

## Implemented change surface and risks

With `KTLSP_TRACE` enabled, `extension.ipc.accepted` records a generic publication
barrier after a validated `analysis/finish` commit has reached the shared Index,
inside the same Workspace access. Its arguments are `provider`, `owner`, worker
`instance`, the opaque `evaluation` token, `publication_revision`, and `source_sha256`.
The digest covers the UTF-8 source string supplied in `analysis/evaluate.params.source`;
the source text and full DTO are not logged. Tracing disabled adds no digest or JSON work.

The barrier requires a committed, still-current contribution and enabled running worker
instance. Pending, failed, stale, replaced, unpublished, and subsequently source-invalidated
evaluations cannot report acceptance. An unrelated provider publication does not invalidate
an otherwise current contribution. Editor probes can wait for this generic event tied to
their provider, owner and source digest, then send a semantic request once, without relying
on a framework diagnostic marker or retrying until a desired semantic result appears.
This is a debug observation at publication time, not persistent readiness, producer closure,
or a promise that later edits, configuration changes, or child exits cannot retract it.
Focused host and real-editor verification for this addition is recorded separately.

| File / area | Current implementation |
| --- | --- |
| `crates/ktcore/src/extensions.rs` and extension contract tests | Preserve session in pending state; narrow validated resume and missing-source-read accessor; test prior-read invalidation and ordinary final commit. |
| `crates/ktlsp/src/extension_ipc.rs` (new) | Wire DTOs/opaque registry, bounded child worker and framing, lifecycle and host-batch dispatch; no MC policy. |
| `crates/ktlsp/src/analysis_extensions.rs`, `analysis_extensions/ipc.rs` | Register remote provider state, queue dirty owners, retain pending transactions, accept/fail batches and expose accepted changes. Replacement-instance checks prevent an old completion/failure from mutating the new worker. |
| `crates/ktlsp/src/analysis_extensions/source_hydration.rs` | Include staged missing-source reads in the existing wanted-source frontier without making IPC readiness a completion wait. |
| `crates/ktlsp/src/workspace.rs` | Short begin/batch/finish wrappers, existing contribution publication and revision invalidation. Existing semantic feature consumers remain shared. |
| `crates/ktlsp/src/lsp.rs`, `lib.rs` | Explicit loader/configuration entrypoint, schedule remote work after source/model/config events, publish diagnostics after completion, shut down owned workers. No child wait in completion. |
| `crates/ktlsp/src/compile.rs` (visibility only) | Reuse the existing Windows ProcessTree through crate-private visibility changes, without changing compile behavior. SidecarClient has a different line protocol and blocking Drop and is not an IPC adapter. |
| `dev/external-extension/src/bin/ipc-child.rs`, `ipc-probes.lua`, `ipc-manifest.json` | New child auto-bin in the existing standalone test crate; the existing corpus extra-probe route supplies deterministic pause/cancel/crash/rename evidence. No shared harness script changes, new crate or second test framework. |

Watch duplicate provider IDs, source retention for remote-only registrations, missing-source
hydration while work is staged, namespace/config changes between batches, and keeping the
restored session's requirement tokens consistent. Child process cleanup must not run from
a Workspace-locked Drop. The source-hydration scheduler and compiled-provider flow already
have independent lifecycles; extending them must not make either wait on IPC. Snapshot cost,
pending-knowledge coverage and editor re-request behavior remain unmeasured risks.

## Later scope ledger

The mc-dev migration ledger reports a frozen legacy baseline of 2,604 passing tests; the
task also reports a 63-diagnostic-code surface and eleven production commands. These are
legacy-scope reports, not host-verified Rust or IPC parity. Its detailed source is
`mc-dev-lsp/docs/rust-migration-inventory.md`; host gaps must remain visible even when the
first Shadow spike passes. None of the rows below expands that spike.

| Scope beyond the first spike | Needed decision / evidence |
| --- | --- |
| Complete mc-dev semantic feature coverage | mc-dev inventories its existing Rust-port/domain behaviors; add missing normalized facts with shared completion/hover/navigation consistency tests. One Shadow case is not coverage of that inventory. |
| Versioned class bytes / structured bytecode | Expose Code instructions, LVT/live-local inputs and exception tables tied to host-issued type/member identities and selected artifact generation, with scope and invalidation checks. Signature-only member facts do not prove instruction/local/expression behavior. |
| Unsaved resource snapshots and tracked resource reads | Support Mixin JSON, Tiny/SRG/TSRG mappings and AW/AT contents from the host's current document/resource model; include positive/negative resource reads, versions and artifact/config generations in atomic validation. Do not let the child independently read stale disk files. |
| Full completion edits/snippets, hover and code actions | Preserve replacement ranges, snippet/plain-text format, placeholders, imports/additional document edits, ordering/docs, hover ranges and validated action application. Decide a negotiated editor boundary only where shared semantic facts cannot express the feature; no arbitrary LSP forwarding in v1. |
| All mc-dev commands | Inventory command IDs and typed arguments/results in mc-dev, then design explicit host advertisement/routing and applied-edit validation. The existing eleven-command surface is outside this spike. |
| Custom language IDs | Explicit AW/AT document registration and same-connection completion/definition/references/diagnostics/action dispatch; define lifecycle/source projection, ownership and cross-language identities. Never route Kotlin buffers through a Java-only adapter. |
| MixinExtras/Lapis, multiple targets and generated behavior | Separate semantic acceptance cases and runtime/compiler evidence; not implied by the IPC transport or basic Shadow equivalence. |

This slice has focused Rust and real-editor evidence, not a new broad workspace/release
acceptance result. The next coherent integrated verification is tracked separately from
this captured build-2 prototype.
