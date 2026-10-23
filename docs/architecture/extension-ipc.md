# Semantic extension IPC v1 — implementation proposal

Status: design draft. The loader, worker and Backend integration below are planned, not
implemented or tested. This work is independent of the ongoing core bug fixes. Production
Minecraft policy stays in mc-dev; ktlsp supplies a general semantic extension host and one
editor-facing LSP connection.

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
repository-selected programs automatically. Registration is not implemented yet. Keep
stdout for framed JSON and drain bounded stderr separately. Restart creates a new provider
instance and invalidates its handles. stdio isolation is not an OS sandbox.

## Minimum wire contract

JSON-RPC 2.0 uses UTF-8 bodies and Content-Length framing, independently versioned from
Rust crate revisions. `initialize` exchanges protocol major/minor, provider identity,
instance and required capabilities. Reject an incompatible major or missing required
capability for that provider only; optional additions need negotiated support. No stable
native ABI is implied. Method names below are the proposed v1 vocabulary.

| Direction / operation | Minimum meaning |
| --- | --- |
| Host → child `initialize`, `shutdown`, `exit` | Version/capability agreement, validated configuration and owned-process lifecycle. |
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
proposed core change is to retain its session identity and add a narrow resume operation:
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

## First real host spike and acceptance

Use the existing external-extension fixture/corpus harness with one target and one
shadow-style field/method whose JVM descriptor is known. Framework policy in this fixture
is test-only, not production MC logic in the host. Compare the child with the linked
provider using the same Java/Kotlin source and one frontend connection.

The critical sequence is deterministic, not a latency threshold:

1. A real loaded child reaches a barrier after reading the target. Record its paused
   evaluation token; do not release it yet.
2. Send an actual ordinary completion request through Backend/the editor connection.
   Assert the useful normal result arrives **before** releasing the child.
3. Deliver an unsaved target edit through that same connection, then release the old
   child response. Assert its generation check rejects it and no stale site/diagnostic
   is published.
4. Allow a fresh evaluation and compare its binding, completion, diagnostic, goto and
   references with the linked result. Apply rename edits and inspect every changed buffer.

Also cover disabled/config-changed behavior, reverse Java/Kotlin target/owner inputs,
static mismatch, incomplete/absent targets, negative lookup followed by source arrival,
dependency/generated-source invalidation, cancel/crash retraction and a fresh restart.
These are acceptance cases to run; a standalone core or framing test cannot establish
actual loader/Backend/editor integration. Record correctness and response ordering first;
release timing/RSS measurements remain separate from functional barrier assertions.

## Proposed change surface and risks

| File / area | Necessary change in the next coordinated implementation slice |
| --- | --- |
| `crates/ktcore/src/extensions.rs` and extension contract tests | Preserve session in pending state; narrow validated resume and missing-source-read accessor; test prior-read invalidation and ordinary final commit. |
| `crates/ktlsp/src/extension_ipc.rs` (new) | Wire DTOs/opaque registry, bounded child worker and framing, lifecycle and host-batch dispatch; no MC policy. |
| `crates/ktlsp/src/analysis_extensions.rs` | Register remote provider state, queue dirty owners, retain pending transactions, accept/fail batches and expose accepted changes. |
| `crates/ktlsp/src/analysis_extensions/source_hydration.rs` | Include staged missing-source reads in the existing wanted-source frontier without making IPC readiness a completion wait. |
| `crates/ktlsp/src/workspace.rs` | Short begin/batch/finish wrappers, existing contribution publication and revision invalidation. Existing semantic feature consumers remain shared. |
| `crates/ktlsp/src/lsp.rs`, `lib.rs` | Explicit loader/configuration entrypoint, schedule remote work after source/model/config events, publish diagnostics after completion, shut down owned workers. No child wait in completion. |
| `crates/ktlsp/src/compile.rs` (visibility only if needed) | Existing Windows ProcessTree is private and compiler-oriented; narrowly expose its existing owned-tree operations for reuse, without changing compile behavior. SidecarClient has a different line protocol and blocking Drop and is not an IPC adapter. |
| `dev/external-extension/`, existing corpus hook, `README.md`, `docs/fork-acceptance.md` | Reuse fixture executable/project and add child mode plus deterministic barrier probes; document the same harness route and actual evidence. No new crate/test framework. |

Watch duplicate provider IDs, source retention for remote-only registrations, missing-source
hydration while work is staged, namespace/config changes between batches, and keeping the
restored session's requirement tokens consistent. Child process cleanup must not run from
a Workspace-locked Drop. The source-hydration scheduler and compiled-provider flow already
have independent lifecycles; extending them must not make either wait on IPC. Snapshot cost,
pending-knowledge coverage and editor re-request behavior remain unmeasured risks.

## Later scope ledger

| Scope beyond the first spike | Needed decision / evidence |
| --- | --- |
| Complete mc-dev semantic feature coverage | mc-dev inventories its existing Rust-port/domain behaviors; add missing normalized facts with shared completion/hover/navigation consistency tests. One Shadow case is not coverage of that inventory. |
| Extension-specific completion/hover results | Decide a negotiated editor boundary only if semantic facts cannot express the feature; define versioned payloads and invalidation first. No arbitrary LSP forwarding in v1. |
| All mc-dev commands | Inventory command IDs and typed arguments/results in mc-dev, then design explicit host advertisement/routing and applied-edit validation. The existing eleven-command surface is outside this spike. |
| Custom language IDs | Explicit registration and lifecycle/source projection rules, ownership of documents and cross-language identities; never route Kotlin buffers through a Java-only adapter. |
| MixinExtras/Lapis, multiple targets and generated behavior | Separate semantic acceptance cases and runtime/compiler evidence; not implied by the IPC transport or basic Shadow equivalence. |

No Rust/Cargo/editor implementation or verification was performed while preparing this draft.
