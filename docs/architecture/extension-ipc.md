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

### Optional project/type facts (next source slice)

The source adds `project-class-facts-v1` with `projectContext` and `typeJvm`.
Focused Rust checks pass; the new real-child route still needs verification. Neither
operation is present in the published release36 binary. Pin a verified host commit
before using them.

`initialize` advertises the optional capability and
`supportedOps:{"project-class-facts-v1":["projectContext","typeJvm"]}`. The child
selects supported operations in its reply. The host stores that intersection per
worker instance and checks every batch. A semantic-v1-only reply remains valid;
unsupported/unselected operations return the existing request error, not an absent fact.

- `{"op":"projectContext"}` selects the subject from the evaluation owner and accepted
  model. Independent facts describe `workspaceUri`, `scope` (raw Gradle `module` and
  `sourceSet`), `sourceRoots`, `outputRoots`, ordered `classpath.entries` with host origin
  IDs/display paths and coverage, and the selected `sdk` identity/full-or-release API.
  The current host cannot attest a workspace URI, so that field remains Unknown.
  Missing scope, entry origin or SDK selection stays Unknown rather than using a guessed
  path or JAVA_HOME. Entry metadata does not grant filesystem authority.
- `{"op":"typeJvm","target":"<host type token>"}` returns independent `binaryName`,
  `classFlags`, editing-oriented `declaredMembers`, positive `abi` metadata and
  `jvmAbiCoverage`. The ABI view preserves all parsed methods/fields, bridge/synthetic
  members, raw flags, signatures, versions, interfaces, InnerClasses and attribute names
  from a checked `ClassFileFacts` input. Raw members receive no invented editing IDs.
  Its `identity.contentSha256` is a lowercase64-character hex JSON string, representing
  the digest of that individual class input's bytes (`.class` or `.sig`), not an archive
  digest or a hash recomputed from ABI JSON. Oversized or unavailable ABI facts are Unknown.

Parsed metadata does not establish whether an input is a full runtime class, compiler
header or selected SDK API stub. Consequently `jvmAbiCoverage` remains Unknown even
when positive raw metadata is available. Source member coverage is separate and must
not certify full generated JVM ABI. This slice cannot construct a complete-only
ClassIndex/MC HostProjectSession and does not provide class bytes, Code/LVT payloads,
catalog pagination, assignability, resources, commands or external read-set caches.

The evaluation retains one shared model Arc, with no full model JSON cloned into each
file payload. Existing model/source/compiled/configuration reads govern resume and
publication; a model change rejects old batches and final contributions. Metadata
projection uses accepted in-memory selections without filesystem reads. No MC-specific
generation counters are introduced.

### Optional current source snapshots

`source-snapshots-v1` adds two independently negotiated `analysis/readFacts`
operations. Select the capability and the operations in the initialize reply:

```json
{"capabilities":["semantic-v1","source-snapshots-v1"],"supportedOps":{"source-snapshots-v1":["sourceInventory","sourceSnapshot"]}}
```

`{"op":"sourceInventory"}` returns `{"sources":[...],"coverage":Fact<null>,"next":null}`.
Large inventories return a byte-bounded page with opaque `next`; continue with
`{"op":"sourceInventory","after":"<next>"}` in the same live evaluation. Cursors
are bound to its read session and exact current scoped inventory. Foreign or stale
cursors are rejected. A partial page has Unknown coverage; only consuming the
chain through `next:null` and Found coverage establishes the complete inventory.
An empty or one-page inventory retains the same fields and adds `next:null`.
The list contains retained Java/Kotlin source snapshots in the evaluation owner's
exact project module/source-set scope, including files without declarations.
It does not enumerate libraries, other source sets, or arbitrary filesystem paths.
Each entry has an opaque `id`, `displayPath`, typed `documentUri`, `languageId`,
and `identity:{"kind":"source","revision":"<host generation>"}`.
The revision is a decimal JSON string identifying the accepted host input; it is
not an LSP document version. Document version is currently omitted.

Coverage is Found only with complete known compiler source claims and every
claimed Java/Kotlin input represented in the accepted snapshot list. Unknown
compiler roles, missing captured inputs, or missing project scope remain typed
Unknown. A partial list cannot prove absence. Coverage does not certify compiler
success, complete language semantics, or generated JVM ABI.

`{"op":"sourceSnapshot","source":"<inventory id>"}` returns a Fact containing
the same descriptor and opaque ID, exact accepted UTF-8 `text`, and lowercase
`sha256` of those text bytes. Unsaved editor content takes precedence. No newline
or Unicode normalization is applied. The host reads its accepted in-memory
snapshot; the child must not open `displayPath`. URI is Unknown for logical or
unavailable paths. Relative display paths are never canonicalized or read.

Source IDs belong to this evaluation and provider instance. Reusing them in
another evaluation, after restart, or as paths is rejected. Inventory membership
reads track additions and deletions, including empty files; identities and bodies
track source revisions. These reads use the existing resume/final-publication
validation together with model and provider configuration. A late response cannot
publish against superseded inputs.

The existing 1 MiB frame, 256-query batch and handle/evaluation limits apply.
Bodies exceeding 128 KiB are typed Unknown; inventory and accumulated escaped
response bytes have a 512 KiB budget. Oversized batches return a small request
error so the child can split queries in the same live evaluation. Each page issues
only its own source handles and optional continuation; the 16,384-handle and
64-metadata-batch evaluation limits remain in force. Resource/class byte continuations
have the separate shared stream quota described below. Read snapshots in bounded multi-query
batches, rather than one batch per file. This slice has no external read-cache
validation, resource/class-byte access, or Minecraft policy.

The fixed `dev/external-extension/source-snapshots-v1.frames` fixture contains
two real serializer responses (inventory, then current non-ASCII Java text).
The host regression compares exact framed bytes. The existing independent
`ipc-child --project-facts --source-snapshots` consumer and `source-snapshots`
editor-probe mode exercise the same protocol. Pin a verified host commit and its
fixture before using this experimental addition.

Source facts preserve owner/annotation/declaration ranges, normalized annotation arguments,
member kind and known signature/dispatch/visibility/synthetic facts for both languages.
Class-valued annotation/type-reference tokens are resolved by the host in the owner scope.
JVM descriptors are host-validated facts, not a license to erase unresolved or generic
types to Object. Facts use distinct Found, Unknown-with-reasons and DefinitelyAbsent
variants. Coverage/inventory reads are explicit; an incomplete list cannot prove absence.

### Exact source locations (`source-locations-v1`)

Negotiate `source-locations-v1` and
`supportedOps:{"source-locations-v1":["sourceLocation"]}` independently of source
inventory or JVM catalog access. With the current evaluation, send
`{"op":"sourceLocation","target":"<host type or member token>"}` through
`analysis/readFacts`. The target must be a type/member identity already issued by
`resolveType`, `declarationType`, `declaredMembers`, `declarationMember` or a checked
requirement binding. Declaration/signature/catalog/runtime tokens and foreign
evaluation tokens are invalid; the child supplies no name, path or guessed JVM
descriptor to select a definition.

When a semantic read records an exact indexed source key whose body is missing,
the host can hydrate that closed project input outside the Workspace lock. It
requires current compiler-task membership, selected scope/origin, accepted model,
source stamp and content hash; current open buffers win. Only requested eligible
files are read. Installing the body invalidates and requeues the old evaluation,
so an initial Unknown cannot publish an obsolete empty result. Namespace coverage
Unknown alone never grants a guessed file read. No source-inventory capability is
needed for this shared semantic hydration.

Found is a single exact normalized source definition:

```json
{"state":"found","value":{"target":"<same token>","documentUri":"file:///project/Target.kt","range":{"start":{"line":2,"character":17},"end":{"line":2,"character":22}},"source":{"identity":{"kind":"source","revision":"<opaque revision>"},"sha256":"<full current source SHA-256>"}}}
```

The range uses zero-based UTF-16 LSP coordinates, converted with the same shared
text index as ordinary navigation from the exact retained source body. Revision
is a host source revision, not an LSP document version. The URI is an authentic
absolute source file URI, not a filesystem write grant. Private members and
overloads retain their exact issued declaration/signature correspondence; no
same-name search, source reparsing, bytecode-line guessing or child LSP proxy is
performed. Normalized JVM projections retain their actual source definition.

Binary-only/non-source inputs, unavailable source bodies, ambiguous correspondence
and unrepresentable source URIs remain typed Unknown; no successful empty
location list is returned. Stale identities remain stale request errors, not
current Unknown. Existing file/model/namespace reads and provider/evaluation
lifetime checks apply through final validation. The source can include current
unsaved text. Reads use accepted in-memory source snapshots and hold no borrow
across child I/O. This operation adds no general all-document version query or
generation/cache-validation contract.

Opening a target in the editor is not required. Issuing its type/member identity
already requires an accepted source snapshot. A not-yet-retained target is
requested through the existing `resolveType` missing-source read set and host
source hydration; its initial result stays Unknown until those inputs arrive.

The focused source/wire regressions and actual child/editor proof for this new
capability must be qualified separately; older source/catalog checks do not prove
the new operation. Existing frame, aggregate fact-response and query/handle limits
apply.

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

Completion stays nonblocking while an enabled IPC provider has queued or valid pending
work for the current owner. The standard `isIncomplete` flag is preserved with ordinary
items and with an empty list; work for another owner alone does not set it. This flag
requests recomputation on further input, as defined by the
[LSP completion specification](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/completion.md).
It does not promise an automatic request when publication occurs. Actual editor verification is recorded in
`pending146-final-green.json` under the fork acceptance RUN.

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

## Explicit provider commands

The server advertises one standard `workspace/executeCommand` command,
`ktlsp.providerCommand`. Provider domain command IDs are not added to the LSP
capability list. Configuring a transport route does not claim that Minecraft or
any other domain feature has its required facts.

An IPC provider opts in using an explicit configuration allowlist:

```json
{"analysisProviders":{"example.provider":{
  "command":["/path/to/companion"],
  "enabled":true,
  "executeCommands":["example.complete","example.reindex"],
  "configuration":{}
}}}
```

The ordinary child initialization offers `provider-commands-v1` and the
configured `commands` list. An opting-in child returns that capability and its
own `commands` list. Only the intersection is dispatchable. Legacy providers
remain usable without this capability. Initialization of the LSP connection
does not start a child or wait for provider I/O.

The editor sends exactly one outer argument:

```json
{"command":"ktlsp.providerCommand","arguments":[{
  "provider":"example.provider",
  "command":"example.complete",
  "context":{
    "workspaceRoot":"/workspace",
    "documentUri":"file:///workspace/Owner.kt",
    "bufferText":"class Owner",
    "documentVersion":7
  },
  "arguments":[{"domainPayload":"unchanged"}],
  "refreshProject":false
}]}
```

`documentVersion` is optional; an unknown host version is never filled with zero.
The workspace and document are selection hints, not filesystem grants. The host
requires the selected workspace, an owned document, matching current text and
any supplied version. The same check is repeated through the command's final
read transaction. Non-file URIs and parent-directory escapes are rejected.

The child receives `analysis/executeCommand`, with an evaluation-scoped token,
the existing `owner`, `source`, `configuration` and declaration handles, the
domain `command` and original `arguments`, and validated host `context`.
The latter contains the document URI, source SHA-256 and available document
version/language/revision. These are host observations, not an LSP version
inferred from a file timestamp. The domain argument array is forwarded without
decoding, lowercasing, or dropping legacy payload fields.

The child may use the negotiated `analysis/readFacts` and `analysis/require`
operations. It cannot publish semantic contributions or issue arbitrary LSP
requests during a command. Its JSON-RPC `result` is returned unchanged, including
any domain result-or-error envelope. The editor's domain adapter remains
responsible for interpreting and applying domain edits with their preconditions.
The host does not synthesize domain `supportedCommands`, project completeness,
Minecraft DTOs, or successful empty results.

Only an enabled provider's current instance can execute. Disable, restart,
configuration changes and source/model/SDK changes invalidate obsolete reads;
resource and binary reads also retain their issued host input guards. Positive
and negative read sets are validated before exposing a result. No workspace or
read-session borrow is held across child I/O. Cancellation stays cancellation;
a cancelled caller leaves an owned worker to drain or close its existing child,
and its result cannot be published later.

Before dispatch, command preflight negotiates permissions and hydrates any
selected closed-source snapshots required by the negotiated source APIs. It
retires that preparatory read and opens a fresh checked evaluation, so populating
the host's own source cache does not force a second client invocation. The
original owner revision, document version and provider instance must still
match. Preparation performs file I/O outside the workspace lock and never sends
the child command; only the final evaluation dispatches it.

`refreshProject:true` is an explicit host preparation step. The provider command
must first be configured and acknowledged, and its root/document context must
still match. The existing project controller is requested once. After a current
accepted refresh, the host opens a fresh evaluation and dispatches the child
command once, preparing source snapshots for the refreshed model first. Failed
model acquisition returns typed unavailability. Cancelling
the wait does not dispatch the command. A refresh is never requested from an old
read transaction, and child command results are never automatically replayed.
Domain adapters can use this outer flag for their reindex/reload operations;
their domain cache policy remains in the companion.

The bounds are 64 configured/acknowledged command IDs, 128 ASCII identifier bytes
per ID, 128 KiB for the outer argument JSON, 256 opaque arguments, eight pending
commands per provider instance, and 512 KiB for the returned result. Existing
1 MiB frames, fact-query limits and evaluation-scoped handle limits still apply.
Host errors use JSON-RPC invalid params (`-32602`), stale content (`-32801`),
cancellation (`-32800`) or unavailable provider/context (`-32002`). Domain typed
errors remain inside the companion's unchanged result envelope.

The existing external-child corpus probe has a `provider-commands` mode. It uses
the same independent child executable with `--provider-commands`, then checks
the complete returned envelope, an applied versioned edit, current unsaved
text, allowlist intersection, source staleness, explicit cancellation, disable/
restart and one real host refresh. Ordinary completion is requested while the
command child is paused. These are generic transport checks; the eleven MC
domain commands require their own companion integration proof.

### Owned resources (`owned-resources-v1`)

The optional capability negotiates `resourceInventory`, `resourceLookup`, and
`resourceSnapshot`. These operations use the evaluation's host-selected source set.
They never interpret Minecraft metadata, process resources, execute Gradle tasks,
or grant child filesystem access.

The host captures actual Gradle resource membership and resource output directories,
ordered compile/runtime classpaths, and selected build configuration inputs. Missing
legacy metadata, unsupported future filters, unreadable inputs, and stale configuration
produce typed unknown coverage. An actual selected resource can remain available even
when future membership is unknown. Runtime resources do not establish source or compiler
parity; executable class bytes use the separate runtime-class capability.

`resourceInventory {after?}` returns `{resources, next, coverage}`. Follow every opaque
`next` cursor; coverage is found only on the last page of a complete current catalog.
Each descriptor contains an opaque `id`, a host-captured `relativePath`, `association`
(`source`, `output`, `compileClasspath`, `runtimeClasspath`, or `configuration`),
`classpathOrder`, `editable`, optional `documentVersion`, typed `documentUri`, and
`identity: {kind: "resource", revision}`. The revision is an opaque host capture
revision, not an LSP document version. Archive entries need no file URI.

One opaque catalog authority validates its entry selectors; inventory size does not
consume one handle per filename. A selector cannot address another evaluation, scope,
catalog, class identity, or filesystem path. Snapshot cursors remain typed and bounded.

`resourceLookup {relativePath, association?}` performs an exact relative-name lookup in
that catalog, without requiring an inventory of every unrelated asset first. Absolute
paths, traversal, and drive-qualified names are rejected. Found results retain every
matching descriptor and ordered classpath association. A definitely-absent result is
possible only with complete current coverage; otherwise absence is unknown.

`resourceSnapshot {resource, after?}` reads an entry scoped by the issued catalog and returns a
typed fact. Its found value contains `identity` (including the full-content `sha256`),
`documentVersion`, full `size`, chunk `offset`, standard `base64`, `utf8`, typed `text`,
and optional opaque `next`. Reassemble chunks in order and verify the full SHA-256.
Small UTF-8 bodies also have found text; empty UTF-8 content is found `""`. Binary
content has definitely-absent UTF-8 text; larger UTF-8 bodies remain available as byte
chunks even when the convenience text field is unknown. No JSON/AW/AT parsing occurs
in the host. Open documents use their exact current buffer bytes, including its actual
newline convention; the host does not synthesize a final newline.

Resource input reads and archive payload CRC checks run outside the workspace lock.
Current scope captures and bounded body caches are reused. The pending and published
read sets retain input guards: buffer edits, addition/deletion/replacement, negative
lookup changes, configuration changes, model changes, and provider lifecycle changes
invalidate the relevant authority. Known-invalid guards also withdraw semantic facts
from copied index views immediately; scheduled host publication removes and requeues
the affected owner. No successful empty result substitutes for missing authority.

The usual frame, aggregate serialized response, metadata batch, handle, and cache
limits apply. Oversized response batches return the existing recoverable split-query
error. The raw payload cache is bounded to 64 MiB per current resource scope; unsupported
sizes are typed unknown rather than partially certified content. The fixed
`owned-resources-v1.frames` fixture is compared byte-for-byte with the actual serializer.

### Selected runtime classes (`runtime-class-bytes-v1`)

Negotiate the capability and each operation in the initialize reply:

```json
{"capabilities":["semantic-v1","runtime-class-bytes-v1"],"supportedOps":{"runtime-class-bytes-v1":["runtimeClass","classBytes"]}}
```

Both operations use `analysis/readFacts` with the current opaque `evaluation` string;
the response is `{"facts":[...]}` in query order. Unnegotiated operations are request
errors. `{"op":"runtimeClass","internalName":"app/Main"}` requests an exact JVM
internal name, without `.class`. Names must be nonempty, at most 4096 UTF-8 bytes,
and contain no empty slash segment, control character, backslash, `.`, `:`, `;`,
`[` or `]`. A name is a selector, never a path or an identity grant.

The host selects the evaluation owner's accepted module/source set and SDK, then
checks the selected SDK first and the ordered **runtime** classpath second. The first
runtime directory/JAR containing the class wins; malformed earlier inputs cannot be
bypassed for later definitions. Ambiguous SDK module definitions are unavailable.
Compile-only artifacts, compiler-private headers, source archives and another installed
JDK are not fallback runtime authorities. Missing/legacy runtime selection, unavailable
bytes, malformed input and negative lookups return
`{"state":"unknown","reasons":[{"code":"BYTECODE_UNAVAILABLE","detail":"..."}]}`.
The detail is diagnostic text, not a stable enum. In particular this operation does
not expose `definitelyAbsent` or certify a complete class catalog. Negative reads
still retain the selected input guards so a later addition invalidates them.

A found `runtimeClass` fact has these exact value fields:

| Field | JSON type and meaning |
| --- | --- |
| `id` | Opaque string used as the next request's `classIdentity`. |
| `internalName`, `originId` | Strings identifying the exact selected binary name and host origin. |
| `contentSha256` | Lowercase 64-character hex string hashing the individual `.class` or `.sig` bytes, not the archive or projected JSON. |
| `byteLength` | Integer: full executable class input length; zero for an API header. |
| `authority` | `"runtime-bytecode"` or `"sdk-api-header"`. |
| `codeAvailable` | Boolean: executable input bytes are available. This does not assert that every method has a Code attribute. |
| `runtimeOnly` | `true` for runtime bytecode, `false` for an API header. |
| `classHeader` | Typed fact containing the checked API header described below; runtime bytecode returns Unknown `MissingFacts`. |
| `hierarchy` | Typed fact of `{superName, interfaces, accessFlags, signature}` for an API header; runtime bytecode returns Unknown `MissingFacts`. |
| `runtimeAbiCoverage` | Found null for the selected runtime class; Unknown `MissingFacts` for an API header. This is not source/generated-output or catalog coverage. |
| `sourceCurrent` | Always `{"state":"unknown","reasons":[{"code":"notSourceAuthority"}]}`. |

The host's opaque `id` binds the evaluation/provider instance, selected scope, origin,
name and content identity under the accepted model and input guards. No generation or
LSP version field is added to this descriptor. Do not reconstruct a token from the
display fields or reuse it across evaluations. Runtime bytes do not establish that
compiled output matches current unsaved source; use source snapshots independently.

For a selected lower-release SDK, the checked `ct.sym` `.sig` can instead return
`authority:"sdk-api-header"`, `runtimeOnly:false`, `codeAvailable:false`,
`byteLength:0`, and Unknown `runtimeAbiCoverage`. Its `contentSha256` still hashes the
actual signature input. `classHeader` uses the existing raw ABI shape: `identity`
(`originId`, `internalName`, `contentSha256`), integer `majorVersion`, `minorVersion`,
`accessFlags`, nullable strings `superName`, `signature`, `sourceFile`, string arrays
`interfaces` and `attributeNames`, plus `innerClasses`, `fields`, `methods` and `annotations` arrays.
Each inner-class entry is `{inner, outer, name, accessFlags}` (`outer` and `name` may
be null); each member is `{name, descriptor, accessFlags, signature, attributeNames, annotations}`
with nullable `signature`. `hierarchy` projects the same superclass, interfaces,
flags and generic signature. An oversized `classHeader` stays Unknown `MissingFacts`.
The SDK reader rejects a `.sig` containing Code rather than granting executable
authority. A class present only in the installed runtime but unavailable in the
selected release is unavailable; it cannot fall through to a dependency definition.

`{"op":"classBytes","classIdentity":"<id>"}` returns a typed fact whose found
value is exactly `{classIdentity, offset, byteLength, bytesBase64, next}`. The first
three fields are the same opaque string, integer byte offset and full integer length;
`bytesBase64` is standard Base64 of up to 256 KiB of original bytes. `next` is an
opaque continuation string or JSON null. Continue using the same `classIdentity`
and `{"op":"classBytes","classIdentity":"<id>","after":"<next>"}` until null.
Check every offset/length/identity and the reassembled full SHA-256 against the
descriptor. Code, local-variable tables, annotations and other attributes are
preserved in the bytes, without host-side rewriting. Foreign, stale or wrongly typed
tokens/cursors are request errors; a cursor for another class cannot select bytes.
An API-header token returns exactly
`{"state":"unknown","reasons":[{"code":"BYTECODE_UNAVAILABLE","detail":"sdk-api-header"}]}`
and does not schedule a raw-byte read. No empty successful byte stream substitutes
for unavailable Code.

The following examples show the actual serializer's field/envelope shapes, with
illustrative lengths and angle-bracket placeholders; they are not captured fixtures.
The first response describes a hypothetical 300000-byte runtime input:

```json
{"jsonrpc":"2.0","id":1,"method":"analysis/readFacts","params":{"evaluation":"<evaluation>","queries":[{"op":"runtimeClass","internalName":"app/Main"}]}}
{"jsonrpc":"2.0","id":1,"result":{"facts":[{"state":"found","value":{
  "id":"<class-id>","internalName":"app/Main","originId":"<host-origin>",
  "contentSha256":"<64 lowercase hex characters>","byteLength":300000,
  "authority":"runtime-bytecode","codeAvailable":true,"runtimeOnly":true,
  "classHeader":{"state":"unknown","reasons":[{"code":"MissingFacts"}]},
  "hierarchy":{"state":"unknown","reasons":[{"code":"MissingFacts"}]},
  "runtimeAbiCoverage":{"state":"found","value":null},
  "sourceCurrent":{"state":"unknown","reasons":[{"code":"notSourceAuthority"}]}
}}]}}
```

```json
{"jsonrpc":"2.0","id":2,"method":"analysis/readFacts","params":{"evaluation":"<evaluation>","queries":[{"op":"classBytes","classIdentity":"<class-id>"}]}}
{"jsonrpc":"2.0","id":2,"result":{"facts":[{"state":"found","value":{"classIdentity":"<class-id>","offset":0,"byteLength":300000,"bytesBase64":"<base64 of first 262144 bytes>","next":"<cursor>"}}]}}
{"jsonrpc":"2.0","id":3,"method":"analysis/readFacts","params":{"evaluation":"<evaluation>","queries":[{"op":"classBytes","classIdentity":"<class-id>","after":"<cursor>"}]}}
{"jsonrpc":"2.0","id":3,"result":{"facts":[{"state":"found","value":{"classIdentity":"<class-id>","offset":262144,"byteLength":300000,"bytesBase64":"<base64 of remaining 37856 bytes>","next":null}}]}}
```

SDK inventory, runtime directory/JAR reads, decoding and hashing run outside the
workspace lock. The evaluation shares one runtime-scope lease and one selected-SDK
lease, rather than creating a native watcher per class. Positive and negative reads
join the normal resume/final-publication validation. Runtime/SDK replacement, directory
membership changes, accepted model/source changes and provider lifecycle changes
invalidate obsolete reads and their published facts. Reusing cached bytes requires
the exact retained identity and current guards; an input change cannot certify old
bytes as current or silently rebind an old handle.

The class-file input bound is 16 MiB; one prepared multi-class byte read also retains
at most 16 MiB. Existing 256-query batches, 16,384-handle limits, 512 KiB aggregate
serialized fact-response budget and 1 MiB JSON body frame limit still apply. Split
oversized batches on the existing recoverable response-budget error. Stream quotas
are **separate** from the 64 ordinary metadata batches per evaluation: each validated
`classBytes`, `resourceSnapshot`, or `jvmCatalog` query with `after` consumes one of 256 shared
continuations. A continuation-only batch consumes no metadata batch; an initial read
or mixed metadata/continuation batch still consumes one. Repeated valid cursors cost
quota again, and failed prepared reads do not refund reservations. The shared stream
budget is 64 MiB of actual serialized JSON-RPC reply bodies for batches containing
continuations, including their envelopes/errors, excluding Content-Length framing;
it is not a decoded-byte allowance. Thus a 16 MiB class can complete its 64 chunks
without consuming 64 metadata batches. Resource payload-cache bounds are separate.

Existing regression coverage is in `runtime_class_tests`, `sdk_api_header_wire_tests`
and `stream_quota_tests` in `crates/ktlsp/src/extension_ipc.rs`, with ordered-reader
controls in `runtime_bytes.rs` and selected full/lower-release SDK controls in
`sdk_runtime.rs`. The independent consumer is
`dev/external-extension/src/bin/ipc-child.rs` (`--host-inputs`), exercised by the
existing `corpus` harness route, `ipc-probes.lua` and `host-inputs-manifest.json`.
The [README host-input recipe](../../README.md) and
[current acceptance record](../fork-current.md) retain the actual immutable62
194/194 semantic-check producer/consumer result, including independent JDK21 jmod,
dependency-JAR and compiled-application byte hashes. That editor run did not exercise
the header-only SDK branch; its focused SDK checks are separate evidence. There is
no new runtime-class golden-frame fixture or claim of Minecraft runtime/remapping parity.

### Selected physical JVM catalog (`jvm-catalog-v1`)

This additive source contract exposes the selected physical class inputs and their
checked raw ABI. Its native checks and actual independent-child catalog result are
recorded in the [current acceptance record](../fork-current.md); the earlier
runtime/source/resource results remain separate. This does not declare the full
eleven-command Minecraft integration ready.

Negotiate the capability and the operations used in the child initialize response:

```json
{"capabilities":["semantic-v1","jvm-catalog-v1"],"supportedOps":{"jvm-catalog-v1":["jvmCatalog","jvmClass"]}}
```

Each operation uses `analysis/readFacts` with the current opaque `evaluation`.
Unnegotiated operations, foreign selectors and invalid cursors are request errors.
Valid but unavailable inputs return typed Unknown. Foreign or wrong-kind tokens
and other invalid reads are fatal to the whole evaluation and can terminate or
disable its provider instance; do not continue that evaluation after the error.
Re-enable/restart establishes a fresh instance. An outer/internal trace reason
may be normalized and is not a substitute for the public JSON-RPC error contract.

`{"op":"jvmCatalog","prefix":"java/util/","after":null}` returns
`{view:"selectedPhysicalJvmInputs", classes:[...], next, coverage, problems,
sourceCurrent}`. `prefix` defaults to `""`; it matches exact JVM internal-name
prefixes, not source qualified names. It is at most4096 UTF-8 bytes, cannot start
with `/` or contain `//`, control characters, backslash, `.`, `:`, `;`, `[` or `]`.
Follow each opaque `next` with the same prefix in `after` until JSON null. A cursor
cannot be reused with another prefix, evaluation or provider instance. Pages are
bounded by serialized bytes, not a child-selected item limit.

Every `classes` descriptor contains:

| Field | Meaning |
| --- | --- |
| `entry` | Opaque selector for this exact physical entry; use it unchanged in `jvmClass`. |
| `internalName` | JVM binary internal name, including literal `$` where present. |
| `originId` | Host-issued selected input origin; it grants no filesystem access. |
| `association` | `"output"`, `"compileClasspath"`, or `"sdk"`. |
| `classpathOrder` | Order within that association's selected inputs; SDK archive order for a full SDK. |
| `authority` | `"compiled-input"`, `"sdk-runtime-input"`, or `"sdk-api-header"`. |

The view enumerates the owner's accepted source-set class outputs, ordered compile
classpath and selected SDK. It preserves duplicate class names and physical entries;
it does not choose a winning definition across output/compile/SDK associations.
Entries sort by internal name, association and per-association order. That sorting
is pagination order, not a combined semantic precedence rule. Do not path-sort,
deduplicate or treat compile-only inputs as runtime bytecode authority.

Only terminal `coverage:{state:"found",value:null}` proves exhaustive membership
of this **selected physical view**, including absence of an entry with the requested
prefix. Intermediate pages and incomplete inputs have Unknown coverage. A complete
name inventory does not prove that every body decodes; `jvmClass` can independently
be unavailable. `sourceCurrent` remains Unknown with reason `notSourceAuthority`:
this view does not certify unsaved source members, unbuilt generated output, complete
source-callable members, or the whole source/generated JVM ABI world.

Missing model/SDK selections, unresolved classpaths, unreadable/unsupported entries
and the200000-class bound prevent complete coverage. With no usable catalog, the
response has an empty list, null continuation and Unknown coverage with reason
`JVM_CATALOG_UNAVAILABLE`; that is not successful absence. `problems` contains
bounded diagnostic strings, not a stable enum. Recognized module-only artifacts
and module descriptors may be excluded from the ordinary class inventory.

For classpath JARs, `jvmCatalog` / `jvmClass` use the selected SDK **home's Java
feature version**, matching `runtimeClass` / `classBytes`. A main-manifest
`Multi-Release: true` selects the highest canonical `META-INF/versions/N/` class
with `9 <= N <= home feature`, including version-only classes; inactive versions
and non-versionable `META-INF` resources do not become class candidates. Duplicate
variants, invalid active class paths, or an unavailable home profile cannot
establish complete coverage. The `compileClasspath` association identifies an
input's provenance: this is raw `selectedPhysicalJvmInputs` authority for exact
JVM/private/bridge ABI, not a compiler front-end or source-callable view. For
example, home21 selects raw MR21 even when a Kotlin compiler reads the base class
or javac `--release 17` reads MR17. Shared source bindings still use existing
normalized type/member facts, and `sourceCurrent` remains unknown. Selected-release
`ct.sym` entries retain their separate `sdk-api-header` authority and never grant
Code. Selection follows the [JDK JAR specification](https://docs.oracle.com/en/java/javase/21/docs/specs/jar/jar.html#multi-release-jar-files)
and [version-configured JarFile](https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/util/jar/JarFile.html).

`{"op":"jvmClass","entry":"<entry>"}` reads precisely the issued entry and
returns a typed fact. Found is
`{entry, authority, abi, sourceCurrent}`. `abi` reuses the raw `class_abi` shape:
checked origin/internal-name/individual-content SHA-256 identity, class versions,
raw access flags, superclass/interfaces/signature, attributes, InnerClasses and
all decoded fields/methods with exact descriptors, signatures and attribute names.
Private, bridge, synthetic and generated entries are not filtered into an editing
member list. No source MemberId, range or shared-binding identity is fabricated.
Decode/name/size failures are Unknown `JVM_ABI_UNAVAILABLE`; an oversized ABI can
remain Unknown `MissingFacts`. Coverage of the catalog is independent of this fact.

A selected lower-release SDK uses its checked `ct.sym` API entries with authority
`sdk-api-header`; a header containing Code is rejected. A normal selected SDK uses
`sdk-runtime-input`. Even when the cataloged input is a real class file, its catalog
selector grants **ABI only**: passing `entry` to `classBytes.classIdentity` fails
with JSON-RPC `-32602`. Obtain a separate `runtimeClass` token for executable bytes
under that operation's selected-SDK/runtime-classpath rules. Compile-only catalog
membership cannot grant Code, and header-only SDK facts cannot provide executable
bytes. Every found catalog ABI still has Unknown `sourceCurrent`.

Enumeration, archive reads, decoding and hashes run outside Workspace locks.
An evaluation retains one catalog/model and shared host-input leases; installation
requires the same accepted model and current guards. Catalog membership and exact
entry reads join the host read set and ordinary resume/final validation. Input
replacement, directory membership, model/source and provider lifecycle changes
invalidate obsolete work. These are evaluation-scoped facts, not external generation
counters, persistent child cache validation or a new assignability operation.

The existing1MiB frame,512KiB aggregate fact-response budget,256-query batches,
16384 handles and64 metadata batches apply. `jvmCatalog` continuations share the
256-continuation/64MiB serialized-reply quota with resource/class-byte continuations;
continuation-only batches do not consume metadata batches. Each class input and
one prepared multi-class ABI read are bounded to16MiB. Split oversized batches
on the existing recoverable split-query error; never reinterpret a quota failure
or Unknown coverage as a complete empty catalog.


### Typed binary declaration annotations

Select `binary-annotations-v1` in the initialize response's `capabilities` array
to include typed declaration annotations in the existing `jvmClass` ABI,
`typeJvm` ABI, and `runtimeClass` API-header representations. This is a
representation capability: it adds no query or `supportedOps` entry and grants
no classpath or executable-byte authority. Without selection, class and member
objects omit `annotations` and retain their existing representation.

Selected ABI class and member objects expose `annotations`, an ordered array
with `runtimeVisible`, a JVM object `descriptor`, and ordered explicit `elements`
(`name`, `value`). Values have stable `kind` tags: `byte`, `char` (UTF-16 code unit),
`double`, `float`, `int`, `long`, `short`, `boolean`, `string`, `enum`, `class`,
`annotation`, and `array`. Scalars use `value`; `long` uses a decimal string to
avoid JSON client precision loss. `float` and `double` use exact lowercase hex
`bits` (8/16 digits), including signed zero and nonfinite values. Enum values
carry `descriptor`/`name`; class values carry `descriptor` (including `V`); nested
annotations carry `value` with `descriptor`/`elements`; arrays carry ordered
`values`. String values containing unpaired surrogates instead carry exact `utf16`
code-unit arrays with the same `kind:"string"`, without lossy replacement or a
Unicode `value`. Defaults, parameter/type annotations and framework meanings are not
expanded. Malformed declaration annotation payloads make that class read unknown
rather than silently dropping values. Existing entry authority, content hash,
source-current Unknown, read guards and runtime byte permission remain unchanged.

### Normalized authored facts (`source-facts-v2`)

Select `source-facts-v2` with `supportedOps:{"source-facts-v2":["sourceFacts"]}`.
Then read `{"op":"sourceFacts"}` in `analysis/readFacts`. Its result is
`{"declarations":[...]}` for the same owner and accepted source snapshot as the
evaluation. Source text and configuration remain in the initial evaluation. The
read works on the first evaluation after initialize; it does not wait for a
second evaluation. Capability and operation acknowledgements are both required.

V2 declarations also carry `parameters`, a knowledge object. A supported ordinary
method/function returns `{"state":"found","value":[...]}` in authored value-parameter
order. Each row has `name`, `range`, `nameRange`, `typeRef`, and `annotations`.
Both ranges are half-open UTF-8 byte offsets in the same accepted source;
`range` covers the complete parameter, including declaration annotations and
modifiers, and `nameRange` covers its authored identifier spelling. Backticked
Kotlin names retain their exact source range while `name` uses the indexed name.
`typeRef` is an existing opaque reference token preserving the indexed type and
generic arguments. Resolve it with the existing `resolveTypeRef` operation;
unresolved identities retain the operation's ordinary Unknown result. Parameter
annotations use the same v2 type references, exact ranges and explicit value
representations as declaration annotations, including nested values and arrays.

A supported zero-arity callable returns Found `[]`. Noncallables, constructors,
Java receiver/vararg/post-name-array forms, incomplete headers, Kotlin extension,
context/suspend/default/vararg/unsupported-owner forms, projections, and unbound
method/class type-parameter references return Unknown for `parameters`. A type
parameter must not resolve as an unrelated nominal class imported under its name.
The list supplies no JVM parameter indices, local slots, generated/default
parameters, or lowering claim. Existing `declarationMember` signature coverage
remains independent. Parameter normalization is bounded to 255 authored entries
and 64 type-reference levels; the existing annotation budgets also apply.

This is additive metadata in the already optional v2 read, with no new operation
or capability. Legacy initial evaluation declarations omit `parameters` entirely.
Consumers supporting older v2 hosts treat an absent parameter field as Unknown.
Foreign snapshot parameters and their annotation references use the same checked
source view/session, source generations and read-only snapshot handles as other
v2 source facts. No standalone parameter identity or contribution authority is
issued. Parameters are retained from accepted trees and indexed parameter types;
reading them does not start a parser.

The initial evaluation retains the legacy declaration/annotation schema for all
children. New value variants are honest `unknown` there, recursively inside
arrays. In particular an escaped or concatenated value is never sent as a
legacy plain string with a false `contentRange`.

The optional read retains declaration `range` as its indexed name identity and
adds `fullRange`, `headerRange`, `nameRange`, nullable `bodyRange`, and `modifiers`
with separate knowledge facts `visibility`, `static`, `final`, `abstract`.
Ranges use UTF-8 source bytes. Java fields share the entire field declaration
range, including multiple declarators. Explicit finality is known; unsupported
implicit Kotlin finality and compiler projections remain unknown.

Annotations add `nameRange`. Values additionally support:

- `{"kind":"annotation","annotation":{...}}`, recursively using the same
  annotation schema and exact name/full ranges. Kotlin constructor syntax needs
  a uniquely resolved host annotation declaration; ordinary or unresolved calls
  stay unknown.
- `{"kind":"integer","type":"int"|"long","value":N}` for checked literal
  values only. Arbitrary constants and arithmetic are unsupported.
- `{"kind":"enum","typeRef":"<host token>","name":"ENTRY"}` only when the
  host resolves one enum owner and one indexed enum entry. Source and binary-stub
  enum facts follow the same index path. Unresolved or ambiguous values stay
  unknown.
- `{"kind":"mappedString","value":"...","segments":[{"decoded":{"start":0,
  "end":1},"source":{"start":42,"end":48}}]}` for decoded quoted literals and
  literal-only `+` folding. `decoded` uses UTF-16 code units; `source` uses UTF-8
  bytes. Each segment is exactly one Unicode scalar or complete escape, including
  both escapes for a surrogate pair. A supplementary scalar cannot be split
  between edits. Quotes and concatenation syntax have no decoded segments.

Unescaped plain strings keep the existing `kind:"string"` and `contentRange`.
Interpolation, text blocks/raw strings, unresolved constants, and expressions
other than literal-only concatenation remain unknown. Java Unicode escapes that
would alter tokenization are conservatively unsupported. The host reruns the
same literal decoder against actual source expressions before accepting decoded
values/maps; range containment alone does not admit a forged mapping.
Normalization bounds annotation input to 64 KiB, 4096 normalized nodes, and 128
recursive levels; mapped strings additionally cap decoded scalars at 16,384.
Unsupported values remain unknown and annotations exceeding the whole-annotation
budget are withheld. Plain strings use their existing direct representation.

The focused `KTLSP_IPC_MODE=source-facts-v2` real-child/Neovim scenario negotiates
this read and checks the first owner evaluation, exact declaration/annotation
ranges, decoded concatenation mapping, nested annotations, and typed Int/Long
values. It waits for host acceptance of that evaluation and returns before the
older source-location and mutation sweeps.


## Optional owning-project input model

`project-inputs-v1` negotiates only `projectInputs` (operation bit 8192).
`analysis/readFacts` accepts `{ "op":"projectInputs", "after":null|opaqueCursor }`.
The host selects the one Gradle project containing the already accepted command/evaluation
owner. Providers cannot supply module names, filesystem roots, Gradle properties or project paths.

The response has `records`, `next` and `coverage`. Pages preserve every supported record;
an oversized record or exhausted quota returns an error rather than an incomplete successful list.
`project` records identify the Gradle project/build, the owning source-set handle, original
capture identity/time/Gradle version, applied plugin classes and an independently observed index
state. The capture survives cache loads; a new Gradle dump gets a new capture identity.
Legacy/invalid capture data stays Unknown. Index state comes from the actual Workspace generation,
completion flags and revision. Its counts explicitly describe unique accepted workspace source inputs
and dependency origins; source-set classpath counts describe ordered entries of that set.
Neither count certifies compiler coverage, and no empty pending queue supplies completeness.

`sourceSet` records issue evaluation-bound opaque scope handles for exported compiler source sets.
`root`, `classpathEntry`, `compilerTask` and `sourceMembership` records preserve configured roles,
entry order, available selected component identities and compiler task identities. Compile,
runtime, class-output, resource-output and provided-artifact roles remain distinct.
KGP logical source sets, their `dependsOn` edges, source/resource roots and current selected files
appear separately as `kotlinLogicalSourceSet`, `logicalRoot` and `logicalMembership` records.
These logical records grant no compiler/runtime authority. Uncaptured KGP models or non-JVM
targets keep whole-project model coverage Unknown. The coverage domain is Java source sets and
Kotlin logical source-set declarations, independently of source/resource inventory and semantic
coverage. An exported JVM compilation list alone never proves every Kotlin logical set is covered.

`sourceInventory`, `resourceInventory` and `resourceLookup` additionally accept optional `scope`
with a source-set handle issued by `projectInputs`. Omitting it preserves owner-scoped behavior.
Existing source/resource snapshot identities retain their issuing scope and generations. Cursors
cannot cross scopes or evaluations. Source membership is derived from captured task selections and
verified stock future-source policy; a shared membership does not choose an authoritative owner.
When `project-inputs-v1` and `projectInputs` are negotiated, source descriptors expose
separate `ownerScope`, `memberships` and `editable` facts. Unknown
authored/generated policy keeps editability Unknown. Resource descriptors preserve their boolean
editability and expose a separate authoritative `ownerScope` fact plus contextual `membershipScope`.
These additional project fields are omitted without that negotiation; legacy source/resource
descriptor schemas and fixed frames remain unchanged.

Scoped source reads record source, selected-scope, same-module membership, missing-selected-file
and model generations. Project model reads also observe the current overlay epoch and namespace.
Resource inventories and absence retain the existing native leases/overlay epoch. Capture runs in
the existing blocking worker; it does no filesystem work or Gradle execution under the Workspace
lock. Runtime/compile class reads, `typeJvm`, semantic resolution and compiler views remain bound
to the evaluation owner; issuing a sibling inventory handle does not merge their classpaths.

A selected source set's resource classpath can contain output directories declared
by another source set of the same project, such as test inheriting main resources.
Captured output-directory declarations identify these entries without enumerating
unselected paths. Their watched absence can be complete when the directory is
empty or has never been created; the LSP does not run a build to create it.
Missing classpath archives and undeclared inputs still make coverage unknown.

Foreign normalized `sourceFacts` requires the separate `source-facts-v2`
negotiation. Snapshot authority and the selected source's compiler scope govern
its normalized declarations; unavailable scopes remain explicitly unknown.

#### Reading normalized facts from another selected source

`sourceFacts` accepts optional `source`, an existing host-issued source token.
Omitting it preserves the evaluation-owner behavior. Obtain other tokens only
through negotiated source inventories, including `projectInputs` source-set
selection. The host shares `source_snapshot` authorization, accepted text and
generation checks with `source_snapshot_declarations`; display paths grant no
authority. The result adds the checked `source` descriptor and `semanticScope`.
An unavailable or ambiguous compiler owner yields unknown semantic scope and no
declarations, rather than a known empty normalized source.

Snapshot declaration handles retain that file's exact definition and revision.
V2 declarations additionally expose authored `sourceKind` (`Function`,
`Property`, `Class`, and the other shared index kinds), so an unsupported
descriptor does not erase whether the authored source is a callable or property.
Type references from its annotations, nested annotations, class values and enum
values retain the same source provenance. `resolveTypeRef` temporarily selects
that file's verified compiler/caller view, using the existing core resolver and
read set. It returns the correct qualified identity as a **read-only** snapshot
type (`readOnly:true`), with its checked `contextSource` descriptor. This is the
resolution context; the resolved type may be declared in a dependency. The original owner
view is restored before processing another operation. Shared membership alone
does not establish an authoritative compiler owner.

`declarationType` and `declarationMember` also accept snapshot declaration
handles and reuse the existing core algorithms in that same source view.
Class types retain read-only snapshot type identities. Members retain read-only
snapshot member identities plus their normalized kind, name, JVM signature,
dispatch, visibility, synthetic status and mismatches. JVM method descriptors
carry parameter and return descriptors; consumers need no Java/Kotlin parser to
populate the supported Mixin member slice. Unsupported signatures stay unknown.

With separately negotiated `sourceLocation`, snapshot declaration/type/member
handles can produce exact locations and accepted-source revision/digest facts.
They do not become ordinary owner-world semantic identities: `typeJvm` and
`declaredMembers` still return explicit `UnsupportedSource` knowledge for
snapshot types. Requirements and other generic identity-consuming operations
reject snapshot identities as read-only, and generic signature reuse still
checks every referenced type against the evaluation owner's view. Runtime bytes, JVM
catalogs, and compile/runtime classpath selection remain bound to the evaluation
owner; project snapshot access does not broaden those operations.

Unsaved source edits, source membership changes, model changes and originating
session checks invalidate these reads through the existing guards. Focused
core/IPC cases cover Java and Kotlin main/test source contexts, own-package
annotation/class names, different nominal parameter/return dependencies, exact
foreign-file method/field descriptors and locations,
unavailable/foreign scopes, owner-view restoration and stale reads. Supporting
broader downstream semantic operations on snapshot handles remains a separate
gap; this transport does not establish global Minecraft semantic completeness.

The existing selected-project command probe also negotiates these source reads,
captures normalized facts and declaration type/member reads for every issued
main/test inventory source, and checks its own snapshot identity, name bytes,
member descriptor/source context, source location URI and accepted-text
digest. It consumes the normalized JSON schema and does not parse source code in
the child. This disposable probe uses groups of eight normalized source reads;
existing frame, handle and evaluation budgets still apply to larger projects.

### Filtered workspace files (`root-files-v1`)

This optional capability independently negotiates `rootFileInventory` and
`rootFileSnapshot` through `analysis/readFacts`:

```json
{"op":"rootFileInventory","filter":{"basenames":[],"extensions":["json","json5"],"excludedDirectories":["build",".gradle","node_modules"]},"after":null}
{"op":"rootFileSnapshot","file":"<issued file token>","after":null}
```

The host selects its registered LSP workspace root containing the current
accepted owner/module. Shared files above the module remain inside that workspace
domain. An unregistered, ambiguous or unavailable owner/root gives Unknown.
Children cannot select a root, path or semantic scope. Workspace file access
does not expand the owner's classpath, source/type identities or executable-code
authority, and the read does not run a build or refresh the project model.

Filters are generic literal basenames, final extensions without a leading dot,
and excluded directory-name segments. All three sets are case insensitive.
Basename or extension matches include a regular file; both positive sets empty
select all regular files. There are no implicit hidden-directory or Gradle
exclusions. Use a separate filter/domain when build or Gradle-cache files are
needed. At most128 literals of256 UTF-8 bytes each are accepted; path/glob syntax
is rejected. Raw filter spelling is echoed and continuation cursors bind that
exact request, catalog revision and evaluation.

An inventory page contains `root`, `filter`, `files`, `next`, `coverage` and
`problems`. Its root is `{identity, uri, rootKind:"workspace", revision}`;
`identity` is an opaque JSON string, `revision` a decimal JSON string, and `uri`
a file-URI string. Each descriptor is `{file, uri, relativePath}`, with an opaque
issued `file` token. `problems` is a string array. Follow every opaque `next`
cursor; only terminal `coverage:{state:"found",value:true}` proves absence or
uniqueness within that domain. Unavailable root/catalog capture returns null root
and Unknown coverage, including when no positive files were available.
Distinct matching paths keep distinct tokens even when their basenames match.

A snapshot is a Knowledge value. Its Found value has `file`, `root`, `uri`,
`relativePath`, `identity:{kind:"rootFile",revision,sha256}`, `documentVersion`,
`size`, `offset`, `text` and `next`. The revision is a decimal string;
`documentVersion` is an i32 JSON integer or null. Size and offset are nonnegative
UTF-8 byte counts. Text chunks preserve Unicode scalar boundaries and are at
most96 KiB, shrinking further for the encoded frame budget. Every chunk carries
the complete text's size, identity and lowercase SHA-256. Reassemble contiguous
chunks and verify that hash. A legitimate empty file has Found empty text;
unsupported or unavailable bodies remain Unknown.
Disk revisions identify the captured catalog; overlays retain the owned document
revision and editor version.

The host arms a native root/ancestor lease before walking the filtered domain,
prunes excluded subtrees, and merges current arbitrary-language buffers including
new unsaved files. Disk changes, additions, deletions, renames and overlays retire
observed reads through the existing checked evaluation/source/model/provider
lifetimes. Ambiguous native events may conservatively retire excluded churn.
Symlinks, unreadable paths, unsupported watch coverage and bounds never establish
a complete empty domain. Current native recursive coverage is implemented on
Windows; unsupported platforms return Unknown.

An acquisition invalidated during the walk discards its entire partial catalog
and tries a fresh lease, at most three times. Logical source/overlay cancellation
stops immediately; exhausted stale acquisition returns the existing `-32801`.
An inner stale read retires that evaluation: the child responds to the outer
evaluation with `-32801` and waits for the host's fresh evaluation. It must not
retry the old read or turn the failure into an empty success. The host uses its
existing requeue only with an observed stale input, cancellation or invalid read;
an unsolicited child error does not authorize a blind retry.

The bounds are8 domains per evaluation,100000 visited entries,16000 matching
files,256 descriptors per page,16 MiB per body and64 MiB of accepted cached root
text. Existing frame, stream, continuation and handle limits also apply. Tokens
cannot be exchanged with source/resource handles or reused across evaluations.
The existing external-child `root-files` corpus scenario exercises this contract;
the fork acceptance record distinguishes authored checks from executed evidence.
