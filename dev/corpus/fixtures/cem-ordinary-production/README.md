# Ordinary CEM production controls

Use the existing `corpus --file` route against a **disposable** copy of the pinned
CreateEnchantableMachinery sources. Do not edit the user's project or regenerate
expected hashes from a different source revision. HEAD alone is insufficient:
the snapshot includes existing user edits. The manifest pins relative paths and
exact source/declaration anchors; it contains no machine paths or cache model.

The separate twelve requests cover:

| Ordinary caller | Expected behavior |
| --- | --- |
| `util/EnchantedItemFactory.kt` | One-argument `getPickaxeItemStack(ItemEnchantments): ItemStack`, excluding the nullable `CompoundTag`/`MovementContext` overload. |
| `content/block/roller/EnchantableRollerBlock.kt` | Smart-cast `blockEntity.setEnchantment(ItemEnchantments)`, selecting the authored `EnchantableBlockEntity` member. |
| `content/block/millstone/EnchantableMillstoneBlockEntity.kt` | Implicit delegated-interface `readEnchantments(CompoundTag, HolderLookup.Provider)`. |

Each case requires useful completion, exact authored declaration navigation, hover
type fragments and the selected signature's parameter count/types. The extra hook
uses the same LSP client after indexing, sends one hover and one signature request
per case, and records their raw first responses. It edits no buffer. Keep this
12-request group separate from the original CEM10 and auxiliary Java8 groups.

Before running, materialize the real dependencies/generated sources in the copy
through normal Gradle tasks. The qualified inputs use JDK21, Kotlin2.4.0,
Create6.0.10-280 and NeoForge21.1.248, including its actual generated source archive
adjacent to the selected binary. Use a complete normally exported model and the
selected SDK's sources; do not enrich/rewrite a cache model to satisfy probes.
Set `JAVA_HOME` to that SDK and `GRADLE_USER_HOME` to an isolated populated cache.
Source archives and full CEM source files are not vendored here.

From the ktlsp repository in Bash, with `cem_copy`, `accepted_ktlsp` and `run_root`
set to absolute paths outside this checkout:

```sh
fixture_bash=("$BASH")
if [[ "${OS:-}" == Windows_NT ]]; then fixture_bash+=(-o igncr); fi
KTLSP_CORPUS_WAIT_INDEX=1 \
KTLSP_CORPUS_EXTRA_PROBES="$PWD/dev/corpus/fixtures/cem-ordinary-production/probes.lua" \
rtk proxy "${fixture_bash[@]}" dev/ktlsp-harness.sh corpus \
  --root "$cem_copy" --file "$PWD/dev/corpus/cem-ordinary-production.json" \
  --release --no-build --bin "$accepted_ktlsp" --run-dir "$run_root/ordinary"
```

`source_sha256`/`target_sha256` pin Neovim `readfile()` lines joined with LF.
`source_raw_sha256` separately pins the actual source bytes, including line endings.
The roller file has mixed CRLF/LF: Neovim selects `fileformat=unix` and retains CR
inside its buffer lines, while `readfile()` normalizes it. Do not compare a buffer
line join to the normalized disk hash or rewrite the fixture to hide that difference.
The extra hook requires **exact full wire text == raw disk bytes**, then retains
the same buffer version, changedtick, disk lines and source hash through each RPC.
A different line-ending checkout is a changed input and must be qualified explicitly.

Inspect `artifacts/summary.txt`, `corpus-results.json`, `corpus-requests.jsonl`,
`ordinary-hover-signature.json` and `trace-events.jsonl`. Missing, null or incorrect
useful results fail the expectation; unanswered/unstarted requests remain unmeasured.
Keep semantic outcomes, source-input failures/missing sources and shutdown separate.
The first actual run measured eight requests before an extra-hook newline
precondition failed; only the four unrequested requests ran after correction.
All twelve matched without retries. The evidence retains both runs, the original
precondition failure and a forced shutdown in the second run. These are concurrent
n1 observations, not a latency distribution or IDEA/projectwide parity claim.

## Separate restored Kotlin readiness control

This optional control adds **one** diagnostic request on ordinary
`EnchantableDrillBlockEntity.kt`. It appends a temporary comment in memory, observes
that document version's delivery, restores the exact original bytes, and uses the
existing `canonical_kotlin_diagnostics` helper to await a current compiler-accepted
store before issuing one final pull. Readiness does not depend on desired diagnostic
contents. A current accepted result with errors on this valid input is a mismatch;
an unavailable generation/receipt remains unmeasured. No semantic polling or timeout
extension is used.

```sh
KTLSP_CORPUS_WAIT_INDEX=1 \
KTLSP_CORPUS_EXTRA_PROBES="$PWD/dev/corpus/fixtures/cem-ordinary-production/restored-kotlin.lua" \
rtk proxy "${fixture_bash[@]}" dev/ktlsp-harness.sh corpus \
  --root "$cem_copy" \
  --file "$PWD/dev/corpus/fixtures/cem-ordinary-production/restored-kotlin.json" \
  --release --no-build --bin "$accepted_ktlsp" --run-dir "$run_root/restored"
```

`ordinary-restored-canonical.json` records both outbound versions, current source
and compiler-input hashes, accepted generation/store ID, raw pull, restoration and
hooks. Correlate its accepted store with the actual completed owned Kotlin native
invocation in the trace. The actual control matched once after clean Kotlin2.4.0
completion and exited cleanly. Java was unavailable because no selected Java buffer
was open; this supplies no Java clean/absence authority and no Mixin coverage.
