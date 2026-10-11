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
- fast, compiler-free diagnostics

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
| Okio KMP, cold | First verified cross-file definition | **0.51 s** | No result within 60 s | ktlsp **>116x sooner** within timeout |

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

The mixed, corpus, completion UI and live Gradle probes require an acknowledged shutdown,
one exit notification and actual server exit code/signal zero. Run
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

`corpus --root <project> --file <manifest.json>` runs read-only completion and exact declaration
probes on a real project. The manifest supplies relative source files, unique anchors, tokens,
and expected declaration files/anchors. `KTLSP_PROBE_SERVER=intellij` uses the same official
plugin configuration as `mixed`, with successful build-tool import required. Source hashes, working-tree
status, first/last responses and bounded indexing retries are recorded; retries are not timing samples.
Set `KTLSP_CORPUS_WAIT_INDEX=1` to require both project and dependency indexing notifications and
check the first response without retries. Missing dependency sources remain a failed coverage check;
the selected semantic probes still run after completed indexing and explicitly record incomplete inputs.
Definition requests use the identifier start; completion requests use its end, or the manifest's
`completion_prefix`, so an unfiltered first page does not obscure the requested member.
Optional `buffer_edits` test partial identifiers, dangling dots, syntax errors in a peer file,
and unknown receivers without saving source. Changes and restoration require outbound versions;
each query uses one response after Neovim's flush. Negative unknown cases also require a useful
known receiver in the same edited state. [dev/corpus](dev/corpus/README.md) includes these controls.
See [the corpus record](docs/fork-corpus.md) for pinned projects, candidate probes and remaining coverage.
Set `KTLSP_CORPUS_EXTRA_PROBES` to an absolute Lua file to exercise an external analysis provider
with the same corpus client before shutdown. The file returns a function receiving
`{repo, client, sources, check, project, artifacts}`. Use `check` for assertions and restore any
unsaved edits; the normal source guards, result report and graceful shutdown still run on errors.
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

`dev/processor_project.lua` creates a separate, compiler-verified KAPT/KSP fixture for the same
`corpus` route. Its `processor-generated.json` manifest checks actual generated Java/Kotlin
declarations, incomplete member completion and paired known/unknown receivers in both languages.
Compile the fixture first; generated targets are hash-pinned and are never handwritten.

`gradle-live` also verifies classpath discovery against a disposable Gradle project that creates
`compileClasspath` from a late `afterEvaluate` callback. The probe requires cold and warm
configuration-cache runs to preserve unresolved-dependency output and reuse the cache entry.

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

`KTLSP_PROCESSOR_RELOAD=1 dev/ktlsp-harness.sh gradle-live` creates the real KAPT/KSP fixture
and keeps one client through v1, v2, disabled processing and restored v1. Use JDK 17 and a populated
Gradle 7.4.2/Kotlin 1.8.21/KSP cache, and omit `--root`. Actual builds and runtime controls accompany
native watch delivery, completion, exact generated-source targets, obsolete-API diagnostics and
preserved unsaved Java/Kotlin buffers. Results are in `artifacts/processor-reload.json`.
Transformed APIs such as Lombok remain covered by their separate compiler-plugin cases.

## License

MIT
