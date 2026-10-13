# ktlsp

A fast, lightweight language server for Kotlin and Java, written in Rust.

`ktlsp` focuses on the editor loop: quick startup, low overhead, useful navigation, and
conservative answers. It uses tree-sitter and an in-memory index instead of keeping a compiler or
JVM on the request path.

## Features

Kotlin and Java support includes:

- definitions, references, implementations, and type definitions
- completion, hover, signature help, highlights, and symbols
- rename, auto-import, and unused-import actions
- semantic tokens, inlay hints, folding, and selection ranges
- call and type hierarchies
- navigation into dependency, JDK, and Android SDK sources
- fast diagnostics, with selected Kotlin compiler checks in the background on Windows

Results are intentionally conservative: when the index cannot prove an answer, `ktlsp` may return
no result instead of guessing. Kotlin type inference, overload resolution, and Gradle modeling are
not yet compiler-complete.

## Install

Download a binary from [GitHub Releases](https://github.com/pepegar/ktlsp/releases), or install
from source:

```sh
cargo install --git https://github.com/pepegar/ktlsp ktlsp
```

With Nix:

```sh
nix run github:pepegar/ktlsp
```

Building requires a Rust toolchain and a C compiler for the tree-sitter grammars.

## Editor setup

`ktlsp` communicates over standard input and output. Configure your editor to start `ktlsp` for
Kotlin and Java files.

Neovim example:

```lua
vim.lsp.start({
  name = "ktlsp",
  cmd = { "ktlsp" },
  root_dir = vim.fs.root(0, {
    "settings.gradle.kts",
    "settings.gradle",
    "build.gradle.kts",
    "build.gradle",
    ".git",
  }),
})
```

Logs are written to standard error. Set `RUST_LOG=ktlsp=debug` for debug logging.

### Zed

Zed's Java and Kotlin support comes from extensions that bundle their own language servers
(Eclipse JDTLS and JetBrains' Kotlin LSP or the community `kotlin-language-server`). To use
`ktlsp` instead, install the extensions for file-type association and syntax highlighting, then
override the language server binary each one launches with the [`lsp`
setting](https://zed.dev/docs/configuring-languages#configuring-language-servers) in
`settings.json`:

```json
{
  "lsp": {
    "jdtls": {
      "binary": {
        "path": "ktlsp"
      }
    },
    "kotlin-lsp": {
      "binary": {
        "path": "ktlsp"
      }
    }
  }
}
```

`jdtls` is the adapter name used by Zed's Java extension; `kotlin-lsp` is the default adapter used
by Zed's Kotlin extension (use `kotlin-language-server` instead if you've switched to the
community server). Overriding `binary.path` replaces the extension's managed server binary with
`ktlsp` while keeping the extension installed for tree-sitter and language detection.

### Claude Code

The repository is also a Claude Code plugin marketplace. The `ktlsp-lsp` plugin connects Claude
Code's built-in LSP tools to `ktlsp` for Kotlin, Kotlin Script, and Java files.

Install `ktlsp` first using one of the methods above and verify the binary is on the `PATH` used to
start Claude Code:

```sh
command -v ktlsp
```

Then add the marketplace and install the plugin from Claude Code:

```text
/plugin marketplace add pepegar/ktlsp
/plugin install ktlsp-lsp@ktlsp
/reload-plugins
```

Claude Code LSP plugins configure a connection to an installed language server; they do not install
the server binary itself. If another Kotlin or Java LSP plugin is enabled, disable it to avoid an
extension conflict. See the [plugin README](plugins/ktlsp-lsp/README.md) for CLI installation,
troubleshooting, local validation, and community-marketplace publishing notes.

## Configuration

`ktlsp` reads Gradle version catalogs and resolves source archives from local Gradle and Maven
caches, Maven Central, installed JDKs, and Android SDKs. Extracted sources and symbol caches are
stored in the platform cache directory.

Java source archives support strict UTF-8 and declared ISO-8859-1 through cached published POM
metadata. Unsupported declarations and undecodable sources are reported as incomplete inputs.
Kotlin and JDK source archives use strict UTF-8.

Useful environment variables:

- `KTLSP_CACHE_DIR` — override writable cache storage
- `KTLSP_JDK_SRC` — point to a JDK `src.zip`
- `KTLSP_ANDROID_SOURCES` — point to an Android sources directory or archive
- `KTLSP_ANDROID_SOURCES_DOWNLOAD=0` — disable Android source downloads

Formatting is opt-in through `initializationOptions.formatting` and delegates to an external
formatter such as `ktfmt`.

## Benchmarks

A July 2026 protocol-level comparison used Pinterest ktlint (multi-module JVM) and Square Okio
(Kotlin Multiplatform), with one cold run and medians of three warm process restarts.

| Workload | Metric | ktlsp | JetBrains Kotlin LSP | Relative result |
| --- | --- | ---: | ---: | ---: |
| ktlint, cold | First verified cross-file definition | **0.51 s** | 11.12 s | ktlsp **21.7x sooner** |
| ktlint, warm | First verified cross-file definition | **0.51 s** | 1.98 s | ktlsp **3.9x sooner** |
| ktlint, warm | Completion latency | **0.8 ms** | 182.2 ms | ktlsp **228x faster** |
| ktlint, warm | References latency | **5.9 ms** | 58.3 ms | ktlsp **9.9x faster** |
| ktlint, warm | Rename edit computation | **5.4 ms** | 2,903.6 ms | ktlsp **538x faster** |
| ktlint, warm | Post-index/import RSS | 1,390.9 MiB | **1,173.9 MiB** | ktlsp uses **1.18x the RSS** |
| Okio KMP, cold | First verified cross-file definition | **0.51 s** | Unmeasured after 60 s observation | No measured comparison |

`ktlsp` becomes useful much earlier and keeps foreground requests fast, but its fully loaded
dependency/JDK index is not always smaller than JetBrains' persisted model. JetBrains also provides
deeper compiler-backed diagnostics, quick fixes, formatting, and project import, so latency is not
a semantic-equivalence score.

See [BENCHMARK.md](BENCHMARK.md) for exact revisions, host details, cold/warm cache rules, feature
results, memory methodology, reproduction steps, and limitations.

The current `dev/bench_initialize.py` starts initialization/indexing timing immediately before
process creation and reports `spawn_ms` separately. Compare old and new binaries with this same
script; its time origin differs from earlier runs that began after process creation. Missing
readiness or an unsuccessful shutdown produces `success: false` and a nonzero exit status. The
client waits for the shutdown response before sending `exit`, and reaps a timed-out process.
`success` describes protocol readiness and process lifecycle; scenario assertions establish
semantic correctness and dependency coverage separately.

On Windows, `memory` records resident working-set samples every 100 ms for one second after
readiness (`--steady-memory-seconds`), their median as `steady_working_set_bytes`, and the OS
`peak_working_set_bytes` through the final pre-shutdown sample. These are server-process values;
the sampling window does not establish long-term stability. Incomplete readiness has no steady
value, and other platforms report measurement unavailability. The distinction follows the
[Windows process memory counters](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters).
Run `python3 -B -W error::ResourceWarning dev/test_bench_initialize.py` for focused shutdown,
readiness and native memory checks, including a real protocol peer process.

## Development

The workspace contains the shared semantic engine (`ktcore`), language server (`ktlsp`), and
static-analysis CLI (`ktcheck`).

```sh
cargo test --workspace
cargo fmt --all -- --check
dev/ktlsp-harness.sh basic
```

Editor-facing changes should also run the smallest relevant harness scenario: `features`,
`library`, `java`, `mixed`, `corpus`, `semantic`, `gradle-live`, or `comprehensive`.
`features-results.json` and `java-results.json` retain each RPC's first raw response and
classify unavailable requests as unmeasured. An observation deadline is not a speed target.
Edited diagnostic probes record the outbound version and source hash before judging the
first target notification. The `java` scenario requires each Java/Kotlin notification to carry
that exact document version. Workspace pull reports also retain the client version across
unsaved changes and close/reopen. A versionless notification from another server has an
explicit correlation assumption; neither it nor a first empty push proves that asynchronous
compiler diagnostics are final.
The library scenario also compiles a plain Java owner/caller pair and checks six
first diagnostic pulls through unsaved edits in both buffers. It covers exact overload
error ranges, valid Object/generic/varargs calls, delivery identity and restoration.
The selected Java source proof preserves unknown compiler-plugin and hierarchy cases.
Its first cold diagnostic cost remains recorded as a performance gap in the fork
acceptance log.
The Gradle reload probe also checks captured Kotlin source/library/JDK inputs. The
project model records the selected compiler runtime and launch policy separately;
captured configuration alone does not prove a compiler ran or diagnosed an unsaved buffer.
On Windows, supported captured tasks also diagnose private overlays of unsaved owned
Java/Kotlin sources, including new buffers. Their accepted results use the ordinary
push/pull diagnostic store; unsupported, failed or stale runs cannot clear it.
The `java` scenario also checks Kotlin body members from Java try-with-resources:
partial selectors and unfinished nested `println`, exact member/local/field navigation,
and outer-field scope before the try, in catch/finally and after it. Valid `var`
resources and same-file factory initializers require useful `Body.string` completion.
It also checks a cursor before an authored selector suffix and 32 nested generic
method results with exact Kotlin navigation. Typed enhanced-for variables and `var`
elements from arrays or invariant `java.lang.Iterable<T>` use the same receiver
inference for Java and Kotlin members. Array method returns, post-name dimensions,
creation and local `var` aliases preserve rank and component types through `clone()`
and indexing. Java array rows offer `length` and `clone` in the first response.
The resource probe keeps the original26 first RPCs,12 array queries and26 alias queries,
and adds8 rank2 queries (6 completions,2 exact definitions). The two existing rank2
guards require useful array members as well as excluding scalar members.
The same scenario adds six first-response SDK checks: array row Object methods,
cloned element members, File array members, and exact Object/File source definitions.
These are recorded separately in `java-array-sdk.json`. Whole compiler controls
cover valid uses and reject protected methods, wrong arity and scalar calls on arrays.
Five separate first responses check Kotlin reads of Java arrays: matrix/row `size`,
`get` and `clone`, indexed/cloned element members, and exact Kotlin member navigation.
Both new buffers retain their current outbound source/version and restore exactly;
`java-array-kotlin.json` records them alongside an unchanged owner snapshot.
Paired unsaved Java/Kotlin changes check current completion, exact navigation and
restoration. An initially unknown receiver can use its already-running JDK source
seed before the first response; useful candidates return without waiting. Set
`KTLSP_JAVA_WAIT_INDEX=1` for a separately labelled first-response-after-indexing
control. The default run checks startup responses with readiness flags unset.
Empty useful results fail. `java-resource-body.json` records reconstructed outbound
edits, both producer wires for alias queries, first RPCs and restoration.
The same `java` scenario checks 13 contextual `var` hint types, including numeric suffixes,
generic arguments, wildcards, overloads and Kotlin producers. Six first-response queries cover
unsaved producer changes, a visible line, invalid initializers and restoration;
`java-context-hints.json` records the exact hints and outbound Java/Kotlin source hashes.
The `semantic` scenario checks receiver/result chains, public `super` overload selection
and KMP navigation. Imported extension aliases also cover typed return chains with
same-named decoys, incomplete completion, unsaved import removal/rebinding, references
and applied rename edits that preserve alias spelling. These alias requests consume
their first response and record buffer delivery and exact edits in
`semantic-extension-alias.json`. Shutdown acknowledgement, exit notification and native
server exit are recorded separately in `semantic-shutdown.json`.
Function-local extensions cover lexical and sibling scopes, typed result chains,
incomplete completion, and unsaved declaration rename/removal/return-type changes in
`semantic-local-extension.json`; every query consumes its first response.
The same scenario checks Kotlin `where` and Java `extends A & B` receiver completion and exact
navigation through unsaved bound swap/removal/restoration, recording first responses and source/wire
identity in `semantic-bounded-receivers.json`.
Shared generic interoperability tests also cover parameterized extension receivers,
inline nominal bounds, class literals and real-stdlib expect/actual declaration shapes.
Java/Kotlin source calls also share a conservative equality/lower/upper constraint
slice for projected arguments such as Class<? extends T>. Unknown captures, upper-only
variables, differing lower bounds and Kotlin where/F-bounds remain coverage gaps; see
the current [acceptance record](docs/fork-acceptance.md) for real-library results and measurements.
Use `corpus` with a completion UI case for real-project blank `super.` acceptance;
retain the applied buffer and verify it with that project's compiler and plugins.

The mixed, corpus, completion UI and live Gradle probes record shutdown acknowledgement,
exit notification and actual server exit separately from semantic results. Run
`nvim --headless -u NONE -i NONE -l dev/nvim_lsp_probe_selfcheck.lua` for fourteen focused
shutdown controls without starting a server or compiler.

`java` covers incomplete member completion, inherited members, Java receivers of Kotlin types,
and an unsaved Kotlin declaration edit observed from an unsaved Java buffer on the same client.
It checks imported superclass identity despite same-name types, exact inherited-member definitions,
valid and invalid call diagnostics, and first responses after unsaved superclass import changes.
It also checks same-name Java fields/methods, callable Kotlin properties, nullable function results,
qualified nested types, signatures, and whole-buffer rename edits through the same LSP connection.
Its override rename probe applies all related interface/implementation and Java/Kotlin call edits,
compares five complete buffers, preserves unrelated methods and overloads, and checks fresh goto.
`artifacts/override-rename-snapshots.json` retains original, expected and actually applied source;
the declaration-only rename regression is a failure even if its edit list is nonempty.
The same probe checks a hidden static method and its bare Java call, applies all four declaration/
Java/Kotlin edits, and preserves the parent and an unrelated same-name method. Its complete
buffers and delivered changes are retained in `artifacts/static-bare-rename-snapshots.json`.
The harness also runs with native Windows Neovim through Git Bash and selects `.exe` binaries.

`library` compiles a disposable Gradle 9.6.1 / Kotlin plugin 2.4.0 project with stdlib 2.2.20 pinned,
then checks stdlib/JDK definitions and opened dependency sources. The first run needs the Gradle
distribution and Maven artifacts or network access; compilation is captured in `library-compile.out`.
`dev/smoke_library.sh` delegates to this same fixture. Unsaved method renames and
deletions must update Kotlin completion and navigation; closing and reopening the source must
restore the disk contents without rewriting the dependency cache.
The same scenario checks `Implement missing methods` across Java/Kotlin source interfaces:
typed proposals after unsaved return changes, actual returned edits applied to both buffers,
exact goto, duplicate/unknown/conflicting requirements and restoration. The initial supported
slice covers public zero-argument primitive/String/Unit methods in validated ordinary source
contracts; broader implementation/override support remains in progress.
`artifacts/library-interface-implementations.json` retains the first responses, exact wire
sources and three compiler snapshots, including the actual applied edits.
The [binary-only Java JAR corpus fixture](dev/corpus/README.md#binary-only-java-jar-members)
uses the same library generator and corpus runner for public method completion from
both Java and Kotlin, with local File and Maven Module dependency variants.

Declared Java source encoding is also exercised through the `corpus` route with incomplete/full
library-member probes and exact materialized source positions; see the
[CEM acceptance record](docs/fork-acceptance.md) for results and remaining input-coverage failures.

`mixed` checks Java/Kotlin member completion, generic receivers, exact definitions and references,
and edits in both unsaved buffers on one client. It applies cross-language renames and checks the
complete resulting source. Maven and JDK 17+ are required: original, edited, and renamed copies
must compile, with compiler logs and source snapshots retained in the artifact directory.
Set `KTLSP_MIXED_MVN` when Maven is outside `PATH`. To run the same assertions through
`intellij-lsp.nvim`, set `KTLSP_MIXED_SERVER=intellij`, `IJLS_PLUGIN` to its checkout,
`IJLS_SERVER` to the server executable, and `IJLS_JDK` to the JDK home. Use isolated XDG cache,
data, and config directories for that temporary plugin; the scenario uses its normal client
configuration and requires a successful Maven import. These checks do not establish full IDEA parity.
Equivalent file URIs are normalized before applying edits; IntelliJ edits retain the plugin's
document-version translation and Neovim's stale-edit checks.
The scenario records normal peer-buffer debounce separately, then flushes Neovim's pending
changes for both buffers and checks the first completion/definition responses without retries.
That explicit test delivery barrier does not establish freshness before the editor sends changes.
After the compiler and RPC checks, `mixed` runs native insert-mode completion in both languages.
It types a dot, checks the actual popup's LSP member and wrong-receiver exclusions, accepts it
through the keyboard, and inspects the exact inserted source. `artifacts/completion-ui.json`
separates protocol time, input-to-popup handoff, and selection-to-text verification; this is not
physical-screen timing. Set `KTLSP_UI_SAMPLES` for cycles per language (default 1). Cycle 1 is
the first after readiness; later cycles are repetitions. These small controls do not establish
real-project latency budgets. The same UI probe supports the temporary IntelliJ client.
Accepted completions also require the expected whole-buffer SHA256. The probe records its
spawned server/editor PIDs and executable in `completion-ui-process.json` for external
memory observation; it records no launch arguments or environment values.

`corpus --root <project> --file <manifest.json>` runs read-only completion and exact declaration
probes on a real project. The manifest supplies relative source files, unique anchors, tokens,
and expected declaration files/anchors. `KTLSP_PROBE_SERVER=intellij` uses the same official
plugin configuration as `mixed`, with successful build-tool import required. Source hashes, working-tree
status, first/last responses and bounded indexing retries are recorded; retries are not timing samples.
Set `KTLSP_CORPUS_WAIT_INDEX=1` to wait for both project and dependency indexing notifications and
check the first response without retries. `corpus-results.json` separates `semantic`, `operations`
and dependency-source `coverage`. Valid RPC responses are matched against the expectation;
an empty positive result is a mismatch. Unanswered, protocol-error and unstarted requests remain
unmeasured. An execution deadline never turns unexecuted cases into passes or semantic failures.
Missing source counts are metadata; a required navigation case still has to resolve correctly.
Definition requests use the identifier start; completion requests use its end, or the manifest's
`completion_prefix`, so an unfiltered first page does not obscure the requested member.
Optional `buffer_edits` test partial identifiers, dangling dots, syntax errors in a peer file,
and unknown receivers without saving source. Changes and restoration require outbound versions;
each query uses one response after Neovim's flush. Negative unknown cases also require a useful
known receiver in the same edited state. [dev/corpus](dev/corpus/README.md) includes these controls.
See [the corpus record](docs/fork-corpus.md) for pinned projects, candidate probes and remaining coverage.
The `gradle-8.14.3.json` and `gradle-8.14.3-incomplete.json` manifests cover Java/Kotlin
navigation and unfinished editing after importing the real Gradle project. Discovery
streams large model output through a temporary file and rejects incomplete project
blocks; it also supports this project's Kotlin DSL warnings-as-errors policy.
For pinned Moshi, set `KTLSP_UI_CASES` to the absolute path of
`dev/corpus/moshi-1.15.2-ui.json` and `KTLSP_UI_SAMPLES=31` with the same corpus route.
For pinned Retrofit, use `dev/corpus/retrofit-2.11.0-ui.json` with that sample count.
For pinned OkHttp4.12.0, use `dev/corpus/okhttp-4.12.0-ui.json`; it checks Java's
typed Kotlin `responseBody.string()` in the original guide, including accepted whole-buffer SHA256.
These check one first-after-readiness and thirty repeated native popup/acceptance cycles,
including the exact whole buffer after accepting each completion.
Spring extension-receiver and incomplete-selector regressions use the same route with
`dev/corpus/fixtures/spring-provider` and the two `spring-provider-reduction*.json` manifests.
`spring-extensions-editing.json` adds unsaved cases against the pinned real Spring root;
see [the fixture instructions](dev/corpus/fixtures/spring-provider/README.md).
Set `KTLSP_CORPUS_EXTRA_PROBES` to an absolute Lua file for custom editing or analysis-provider checks
with the same corpus client before shutdown. The file returns a function receiving
`{repo, client, sources, check, project, artifacts, request_outcomes}`. Use `check` for assertions and restore any
unsaved edits; the normal source guards, result report and graceful shutdown still run on errors.
For pinned Retrofit, use `dev/nvim_retrofit_mixed_probes.lua` with
`dev/corpus/retrofit-2.11.0.json`. It changes Java's `Invocation.service()` return type
in memory, checks Kotlin completion/navigation, verifies exact cross-language references,
applies a four-file rename and compares every complete buffer before restoring it.
Use JDK 8 sources for `KTLSP_JDK_SRC` (or JDK 8 as `IJLS_JDK` for IntelliJ) and JDK 17
for Gradle, matching the project's selected main toolchain. This includes completion inside
braced Kotlin string templates across unsaved Java return-type changes. The focused
checks pass; broader coverage is tracked in [the acceptance record](docs/fork-acceptance.md).
For the pinned CEM source-archive watch case, run `corpus` with
`dev/corpus/cem-source-watch.json`, `KTLSP_CORPUS_WAIT_INDEX=1`,
`KTLSP_CORPUS_EXTRA_PROBES=$PWD/dev/nvim_cem_source_watch.lua` and
`RUST_LOG=ktlsp=debug`. It sends one unchanged selected NeoForge source-JAR
notification and awaits the explicit server classification before a normal RPC
and shutdown. `source-watch-proof.json` records this separate finite watch case;
it adds no requests to the ordinary completion/navigation corpus and supplies no
latency distribution. Missing or ambiguous classification remains unmeasured.

Use `dev/corpus/retrofit-2.11.0-template-editing.json` with the same corpus route for
partial template members, missing closing braces/quotes, and literal-negative controls.
Those cases edit buffers only and preserve the pinned source bytes.
The [named-call fixture](dev/corpus/fixtures/named-call-binding/README.md) uses this
same route for reordered arguments, omitted defaults, overload selection, exact
Java/Kotlin definitions, and one-file unsaved parameter-name edits. Its 47 first
requests have 65 shared semantic expectations for ktlsp and IntelliJ.
The [Java diagnostic reduction](dev/corpus/fixtures/java-source-diagnostics/README.md)
keeps closed-owner member errors and the remaining missing type/import expectations
in the same corpus route.
Its alternative metadata fixture checks Gradle's default JDK selection and conservative
fork/custom-task controls without configuring a project toolchain.
`check(name, ok, detail, kind, outcome)` defaults to a semantic assertion; pass the request outcome
from `nvim_lsp_probe.request_outcome` and register hook requests in `request_outcomes`.
For example, `KTLSP_CORPUS_EXTRA_PROBES=/tmp/provider/probes.lua dev/ktlsp-harness.sh corpus
--root /tmp/project --file /tmp/provider/manifest.json --bin /tmp/provider/server --no-build`.
This hook enables shared completion, diagnostics, navigation, references and applied-rename
checks; provider behavior requires an actual run and is not established by registration alone.

`dev/jvm_projection_project.lua` generates a disposable four-buffer Kotlin/JVM fixture for
this route. Its `dev/corpus/jvm-projections.json` manifest checks the Java exported suspend
alias and Kotlin authored name, including first responses while the declaration file is closed.
`dev/nvim_jvm_projection_probes.lua` adds signature help, exact references, applied whole-buffer
renames, incomplete calls and delivered unsaved annotation/facade edits. Generate and run it with:

```sh
project=$(mktemp -d)
nvim -l dev/jvm_projection_project.lua "$project"
KTLSP_CORPUS_WAIT_INDEX=1 \
KTLSP_CORPUS_EXTRA_PROBES="$PWD/dev/nvim_jvm_projection_probes.lua" \
dev/ktlsp-harness.sh corpus --root "$project" --file "$PWD/dev/corpus/jvm-projections.json"
```

The fixture's baseline and both renamed snapshots have independent Kotlin 1.9.23/javac 17
compiler and runtime controls; editor assertions alone do not establish general JVM lowering.
The same corpus route includes a compiler-verified Gradle fixture generated by
`dev/compiler_plugin_project.lua`, with serialization, all-open, no-arg and Kotlin/javac Lombok.
`dev/corpus/compiler-plugins.json` exposes the current generated-API completion gaps.
Set `KTLSP_UI_CASES` to an absolute UI manifest path to run the existing native popup and
accepted-text probe after corpus requests; `compiler-plugins-ui.json` checks Kotlin's generated
getter property. See [the fixture instructions](dev/corpus/README.md) for pins and remaining checks.

The saved Java-only observed-build probe uses the same corpus client and a fresh Gradle
8.10.2/Lombok 1.18.32 fixture. Run from Git Bash with native Windows Neovim on `PATH`,
JDK 17 selected by `JAVA_HOME`, cached dependencies and an accepted release server:

```sh
export JAVA_HOME="C:/path/to/jdk-17"
accepted_ktlsp="C:/path/to/accepted/ktlsp.exe"
fixture_parent="$(mktemp -d)"
fixture="$fixture_parent/project"
rtk proxy nvim -l dev/compiler_plugin_project.lua "$fixture" --java-only
KTLSP_CORPUS_WAIT_INDEX=1 \
KTLSP_CORPUS_READY_TIMEOUT_MS=180000 \
KTLSP_CORPUS_EXTRA_PROBES="$PWD/dev/nvim_observed_java.lua" \
rtk proxy bash -o igncr dev/ktlsp-harness.sh corpus \
  --root "$fixture" --file "$PWD/dev/corpus/observed-java.json" \
  --release --no-build --bin "$accepted_ktlsp" --run-dir "$fixture_parent/run"
```

The saved Java-only regression passes 99 checks in checkpoints 27 and 28, including
the compiler-path preparation change. It requires a real
selected `:compileJava` execution before the first generated
`getName` completion, then checks String inference, configuration changes, dirty-buffer
retraction and fresh rebuilds. Source-less navigation and rename remain unavailable;
compiler-producer closure remains unknown. `lombok.config` stops ancestor configuration
from affecting the fixture. The default mixed fixture and its twelve generated-API
completion gaps remain separate, as does useful generated completion during unsaved edits.
This run verifies correctness; latency/RSS and complete process-tree observations need
separate measurements. See [the acceptance record](docs/fork-acceptance.md).

`dev/processor_project.lua` creates a separate, compiler-verified KAPT/KSP fixture for the same
`corpus` route. Its `processor-generated.json` manifest checks actual generated Java/Kotlin
declarations, incomplete member completion and paired known/unknown receivers in both languages.
Compile the fixture first; generated targets are hash-pinned and are never handwritten.

`gradle-live` also verifies classpath discovery against a disposable Gradle project that creates
`compileClasspath` from a late `afterEvaluate` callback. The probe requires cold and warm
configuration-cache runs to preserve unresolved-dependency output and reuse the cache entry.
Both runs also execute the production compiler loader factory and verify that closing it preserves
another loader's pending resource; the ordinary cached-loader control must reproduce the failure.

The editor waits for the project-indexed event before the first request for each of four project
completions and records client start-to-ready time. An unavailable event is `UNMEASURED`; the
observation timeout is separate from a latency budget.

`KTLSP_GRADLE_RELOAD=1 dev/ktlsp-harness.sh gradle-live` uses a disposable mixed project with
Gradle 9.6.1 and Kotlin 2.4.0. Set `GRADLE_USER_HOME` to a populated wrapper/plugin cache and
`JAVA_HOME` to JDK 17 or newer. Four offline builds add, update and remove compiled dependencies
with source JARs, plus ordinary Gradle-generated sources under standard and custom directories.
The same editor connection checks useful completion, exact definitions, diagnostics and preserved
unsaved Kotlin changes. Removal requires a previously useful result. The fixture remaps `:app`
to `modules/editing`; it requires its generated project, so omit `--root`. Results and build logs
are recorded in `artifacts/gradle-reload.json` and `artifacts/gradle-phase*-compile.out`.
Set `KTLSP_GRADLE_RELOAD_AUTOWATCH=1` to require Neovim's native watched-file notifications;
the default is an explicit standard-notification control.
The probe honors `KTLSP_JDK_SRC`. Set it to the Java compile toolchain's `lib/src.zip`
to check Java's obsolete dependency-member diagnostics against selected classfiles and
their actual JDK ancestors; the default tiny JDK fixture covers ordinary reload behavior.
It also checks the Kotlin task's Java/Kotlin source inputs, configured library classpath
and selected JDK before and after the dependency version changes, saving the raw model.
The Kotlin obsolete-member assertion arms observation before one unsaved edit, waits
for an accepted compiler receipt for the actual transmitted text, then performs one
standard diagnostic pull. Its result ID must identify the same analysis, source and
store generation. Full-text encoding preserves the buffer's LF/CRLF/CR format and
final-newline setting. The original obsolete-member error remains required: unavailable
observations are unmeasured, while certified wrong or empty results are mismatches.
Compiler inputs are fingerprinted off the interactive path and reused with precise
per-scope native watches. Compiler processes and private outputs stay off completion
requests. Arbitrary plugin options, unmodeled flags and dirty visible sources in other
source sets are unsupported. A successful selected pass can clear its captured files;
it does not certify general compiler-plugin or generated-source closure.

`KTLSP_PROCESSOR_RELOAD=1 dev/ktlsp-harness.sh gradle-live` creates the real KAPT/KSP fixture
and keeps one client through v1, v2, disabled processing and restored v1. Use JDK 17 and a populated
Gradle 7.4.2/Kotlin 1.8.21/KSP cache, and omit `--root`. Actual builds and runtime controls accompany
native watch delivery, completion, exact generated-source targets, obsolete-API diagnostics and
preserved unsaved Java/Kotlin buffers. Results are in `artifacts/processor-reload.json`.
The model selects the public main Kotlin compilation independently of auxiliary KAPT/KSP tasks.
After each build the probe orders current model/shared analysis refresh before one acceptance sweep;
Kotlin pulls correlate the transmitted unsaved text with accepted source/model/store generations.
Phase 1 and 2 receipts preserve all three flushed buffer texts and wire identities before the canonical
Kotlin pull, together with the validated refreshed cache and selected task inputs.
All four phases preserve the exact selected source inventories and raw disk/buffer/wire bytes in
`artifacts/processor-selected-sources/phase-N`, before a later clean can remove generated files.
Byte export has separate timing and availability metadata; it adds no semantic checks or requests.
Producer scopes publish located Kotlin errors but cannot certify clean results or unsaved regeneration.
Java producer diagnostics remain an open case. Missing observations are recorded as unmeasured.
Transformed APIs such as Lombok remain covered by their separate compiler-plugin cases.

## License

MIT
