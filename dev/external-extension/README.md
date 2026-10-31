# External provider integration fixture

This independent Cargo workspace composes ktlsp's public
`Backend::with_analysis_providers` API with a test-only provider. It proves the
registration boundary available to a separately maintained Rust project. The
stock ktlsp executable is unchanged. This is executable composition, not runtime
loading of a DLL or a second LSP connection.

The provider reads host-normalized `SourceAnnotation` and `DeclaredMemberFacts`,
resolves the annotation type and `@Mixin(Target.class)` target through the shared
index, and binds `@Shadow(prefix = "shadow$")` using the exact authored JVM
signature and dispatch. It supplies no target identities, invented signatures,
feature-specific completion results, or diagnostic suppression.

Authored owners use `declaration(range)` followed by `declaration_type(&id)`;
the provider does not reconstruct a qualified type name from source spelling.
`TypeId::package()` preserves the configured exact package filter for nested owners.

The committed Java annotation declarations are a reduced API subset. They are
only a convenient unit-test input and implement no bytecode transformation.
The native integration control must instead use an actual Mixin artifact:
compile the `fixture/*.java` files with
`javac -proc:none -classpath "$MIXIN_JAR"`, excluding the reduced declarations.
For the editor run, replace those two reduced declarations in the disposable
project with exact `org/spongepowered/asm/mixin/{Mixin,Shadow}.java` entries from
the matching sources JAR. Record both artifact hashes and the source-entry
hashes with the result; do not claim a reduced API proves the upstream library.
Expected prefix stripping and exact member selection also match
`mc-dev-lsp/mcdev-core/src/test/kotlin/io/github/mcdev/core/mixin/ShadowAccessorInvokerTest.kt`
(`shadowMethodValidationPassesForValidTarget` and
`shadowPrefixResolutionStripsPrefix`). A Mixin/NeoForge runtime launch is outside
this fixture. Multiple targets, aliases, generated targets, MixinExtras and Lapis
remain unproved here.

Use an external output directory and a disposable copy of `project/`:

```sh
cargo test --manifest-path dev/external-extension/Cargo.toml --target-dir /tmp/ktlsp-external-target
cargo build --manifest-path dev/external-extension/Cargo.toml --target-dir /tmp/ktlsp-external-target
cp -R dev/external-extension/project /tmp/ktlsp-external-project
javac --release 17 -proc:none -classpath "$MIXIN_JAR" -d /tmp/ktlsp-external-classes \
  /tmp/ktlsp-external-project/fixture/*.java
KTLSP_CORPUS_WAIT_INDEX=1 KTLSP_CORPUS_EXTRA_PROBES="$PWD/dev/external-extension/probes.lua" \
  dev/ktlsp-harness.sh corpus --root /tmp/ktlsp-external-project \
  --file "$PWD/dev/external-extension/manifest.json" \
  --bin /tmp/ktlsp-external-target/debug/ktlsp-external-shadow-fixture --no-build
```

Set `KTLSP_CACHE_DIR` outside the repository for the Rust test. On Windows use the
same existing native harness controller with this binary, manifest and EXTRA
probe; append `.exe` to the executable path. Do not run editor probes on the
committed fixture because applied renames and source edits must use disposable
buffers.

The EXTRA probe checks useful prefixed completion with the target descriptor and
exact suffix replacement, shared declaration/call navigation, exact references,
and an applied rename of the target, shadow and ordinary call. Whole-buffer
inspection must preserve the `shadow$` prefix and annotation string. It then
checks provider disable/re-enable, package-configuration changes, an unsaved
`@Mixin` annotation switching between two exact targets, unsaved target removal
and shadow rebinding. Missing coverage yields an incomplete hint, never
a false missing-target error. `external-shadow-proof.json` retains RPC outcomes,
renamed buffers and change-delivery versions. The Rust test exercises actual
annotation extraction and target-edit invalidation without injected host facts.

Verified on Windows with JDK 21 (`javac --release 17 -proc:none`), against
`net.fabricmc:sponge-mixin:0.15.2+mixin.0.8.7`. The binary artifact SHA-256 is
`1d45cfe3ae4a2eab38dc74276803748cf799088986260d6d912e50ddb35d15c5`;
the matching sources JAR is
`4b62970960c14df552d1855a1d21366dd8676b3bde129ddbd064a80d70e7f21f`.
The editor used exact upstream annotation sources, not the reduced declarations:
`Mixin.java` SHA-256 `513002b277d74e2fdcc3dc6409bede0cb27e62966582c8e9674a5ad38cfba7f6`,
`Shadow.java` SHA-256 `e136a503ac061e9793d95313ec8ce0e09ddeb22d15b95e530185ef9678142ac8`.

The independent crate test passed. Real Neovim passed both original RPCs and all
14 EXTRA RPCs, including 36 semantic checks with no mismatch, unmeasured or
unrun result. Saved renamed buffers preserve both `shadow$doubled` uses, the
`"shadow$"` annotation argument and the unrelated target. All six owned native
stages exited zero with EOF, no forced cleanup, no live owned process, and stable
input guards. Evidence is under the optional local directory
`C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1/external-shadow86-1/`:
`inputs.json`, `receipt.json`, and `editor/artifacts/external-shadow-proof.json`.

The later nominal group also verifies ordinary class/interface reference identities
and arrays, including signature equality and source/configuration invalidation.
An independently built executable on the published499e98e checkpoint passed the
unchanged2 original +14 primitive +34 nominal requests (50 requests/98 checks).
Evidence: `C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1/external-nominal89-3/receipt.json`
and `verified-summary.json`. These results establish the fixture scope, not IDEA
parity or production framework support.

The new `shape-probes.lua` wrapper runs the existing `probes.lua` first, then a
separate40-request group for enum, record, named-object and mapped-builtin member
signatures. Use the same `corpus` command above with
`KTLSP_CORPUS_EXTRA_PROBES` pointing to `dev/external-extension/shape-probes.lua`.
Compile the Kotlin fixtures with the selected Kotlin compiler against the same
Mixin artifact and Java fixture outputs; retain compiler/artifact identities.
The additional group records `external-shape-proof.json` separately. The combined
original50 and additional40 requests passed on the25-2 checkpoint:90 first
responses and144 semantic checks,0 mismatch/unmeasured/not-run, clean shutdown
and stable input guards. Evidence is `external-public132-25/editor-receipt.json`
under the run directory linked from [the current acceptance record](../../docs/fork-current.md).
The independent crate's4 tests and normal build also passed against checkpoint26
with its own unchanged lock (`external-public134-26/cargo-receipt.json`). These
are recorded checkpoints, not a stable API promise. The later exact-declaration
consumer was rebuilt with its original own lock:4 tests and normal build pass,
including a nested owner and exact package-filter withdrawal/restoration. Its
independent executable also passes the unchanged90 first-response requests and
144 semantic checks with clean shutdown, stable guards, EOF and zero live/forced
owned processes. Evidence: `external-public140-30/cargo-receipt.json` and
`external-public140-30/editor-receipt.json` under the same run directory.
Existing manifests/probe bytes and their denominators stay unchanged.
The focused `KTLSP_IPC_MODE=source-locations` IPC group uses the same child and
probe with `source-locations-v1`. Its initial type/member reads target a Java file
that has never been opened or attached to the LSP client, and compare its exact
file URI, UTF-16 ranges and SHA-256 against disk bytes. It then opens that target,
adds unsaved non-BMP prefixes, and checks fresh locations after stale-evaluation
cancellation and the accepted model refresh. The proof records the initial
unopened state separately in `extension-ipc-proof.json`; this describes the probe,
not a completed run or production MC definition integration.
Nominal identities do not establish generated enum/record members or
object-owned callable lowering. Nullable, generic and value-class ABI shapes
remain unsupported. Production Mixin integration, runtime/remapping, MixinExtras,
Lapis and IDEA parity remain unproved.

Three focused modes reuse `ipc-probes.lua` and the existing corpus harness.
They return before the older source-location, cancellation, rename and command
sweeps. Use a reviewed matching server/child pair and a fresh barrier directory
for each run; the following commands do not build either executable. The harness
retains its one manifest-local completion/definition baseline, using neither the
broader corpus manifests nor the older extension sweeps.

`KTLSP_IPC_MODE=source-facts-v2` uses the small disposable IPC project and
`ipc-manifest.json`. It appends an annotation fixture only to the editor buffer,
reads `sourceFacts` on the first owner evaluation, and waits for host acceptance.
`source-facts-<evaluation>.json` records the initial legacy declarations and the
negotiated result. Assertions require exact declaration/modifier and annotation
ranges, nested annotations, typed Int/Long values, a decoded escaped string
concatenation with a whole-escape UTF-16/source-byte map, and honest legacy
Unknown downgrade. The child never parses Java or Kotlin.

```sh
KTLSP_CORPUS_EXTRA_PROBES="$repo/dev/external-extension/ipc-probes.lua" \
KTLSP_IPC_MODE=source-facts-v2 KTLSP_IPC_CHILD="$sealed_child" \
KTLSP_IPC_BARRIER="$scratch/source-facts-barrier" \
rtk proxy bash dev/ktlsp-harness.sh corpus --root "$ipc_project" \
  --file "$repo/dev/external-extension/ipc-manifest.json" \
  --bin "$sealed_server" --no-build --run-dir "$scratch/source-facts-editor"
```

`KTLSP_IPC_MODE=project-inputs` uses the existing disposable Gradle reload fixture
and `host-inputs-manifest.json`, prepared as in the root README's host-inputs
recipe. The copied Java IPC inputs belong under
`modules/editing/src/main/java/fixture`. Before starting the harness, add these
two small test sources so the selected test compiler scope contains useful Java
and Kotlin members:

```sh
rtk proxy mkdir -p "$project/modules/editing/src/test/java/fixture" \
  "$project/modules/editing/src/test/kotlin/fixture"
rtk proxy python3 -c 'import pathlib,sys; r=pathlib.Path(sys.argv[1]); (r/"src/test/java/fixture/ProjectInputsTest.java").write_text("package fixture; public class ProjectInputsTest { public int testOnly(int value) { return value; } }\n"); (r/"src/test/kotlin/fixture/ProjectInputsKotlinTest.kt").write_text("package fixture\nclass ProjectInputsKotlinTest { fun kotlinTestOnly(value: Int): Int = value }\n")' "$project/modules/editing"
KTLSP_CORPUS_WAIT_INDEX=1 KTLSP_JDK_SRC="$JAVA_HOME/lib/src.zip" \
KTLSP_CORPUS_EXTRA_PROBES="$repo/dev/external-extension/ipc-probes.lua" \
KTLSP_IPC_MODE=project-inputs KTLSP_IPC_CHILD="$sealed_child" \
KTLSP_IPC_BARRIER="$scratch/project-inputs-barrier" \
rtk proxy bash dev/ktlsp-harness.sh corpus --root "$project" \
  --file "$repo/dev/external-extension/host-inputs-manifest.json" \
  --bin "$sealed_server" --no-build --run-dir "$scratch/project-inputs-editor"
```

This mode sends one `workspace/executeCommand` request, allowing the host to
hydrate and requeue internally. It does not issue a second client command to
obtain complete inputs. The project and binary modes deliver the final owner
buffer before enabling the provider, then wait for its existing
`extension.ipc.accepted` publication with the exact owner path and source digest.
This readiness includes host-managed source hydration/requeue and open-file
indexing; the initial global project-index notification precedes these newly
opened inputs. No command is issued or retried during this wait. After binary
capability selection, readiness must come from the new provider instance.
`extension-command-proof.json` records this readiness, captured model
and index state, complete main/test source/resource inventories, exact snapshots
and current identities, the unsaved command-owner text, and source-scoped
normalized types/members. Required members are `Target.twice`, Java
`ProjectInputsTest.testOnly`, and Kotlin
`ProjectInputsKotlinTest.kotlinTestOnly`, each with descriptor `(I)I` and the
correct class identity. Unknown or missing results fail these cases.
`KTLSP_IPC_PROJECT_INPUTS=1` with `provider-commands` remains an alias for this
focused path.

`KTLSP_IPC_MODE=binary-annotations` uses that same Gradle fixture. Set this mode
when generating it as well as when running the harness. The existing generator
copies the committed `CustomPoint.class` into a distinct annotation-only fixture
JAR, which phase 0 selects through `compileOnly` alongside its normal V1
implementation dependency. The existing `fixtureJars` preparation builds that
JAR without selecting the V2 `Versioned` API; no new Java compilation or parser
is introduced:

```sh
# Run the root README's existing Gradle fixture preparation with this mode:
KTLSP_IPC_MODE=binary-annotations rtk proxy nvim --headless -u NONE \
  -c "lua dofile('dev/gradle_reload_project.lua').write('$project', vim.fn.getcwd())" -c qa
# Then run its existing fixtureJars/:app:classes and IPC source-copy steps.
KTLSP_CORPUS_WAIT_INDEX=1 KTLSP_JDK_SRC="$JAVA_HOME/lib/src.zip" \
KTLSP_CORPUS_EXTRA_PROBES="$repo/dev/external-extension/ipc-probes.lua" \
KTLSP_IPC_MODE=binary-annotations KTLSP_IPC_CHILD="$sealed_child" \
KTLSP_IPC_BARRIER="$scratch/binary-annotations-barrier" \
rtk proxy bash dev/ktlsp-harness.sh corpus --root "$project" \
  --file "$repo/dev/external-extension/host-inputs-manifest.json" \
  --bin "$sealed_server" --no-build --run-dir "$scratch/binary-annotations-editor"
```

For an already generated binary fixture, rerunning
`dofile('dev/gradle_reload_project.lua').phase(project, 0)` with
`KTLSP_IPC_MODE=binary-annotations` installs the distinct JAR task and restores
the V1 phase-0 dependency selection. Then rerun its existing
`fixtureJars :app:classes` preparation; the server and child binaries are reused.

The same current child first reads the unselected ABI, then explicitly selects
`binary-annotations-v1` in its next initialize response. No new read operation is
negotiated: both commands use `jvmCatalog`/`jvmClass`. Assertions compare the
unchanged physical hash/origin, omitted unselected annotation fields, exact
selected class/field/method annotation values, nested/array/enum/class values,
lossless long/floating bits and UTF-16 units. Runtime lookup of the compile-only
class must return `BYTECODE_UNAVAILABLE` in both cases, granting no executable
class identity. This mode also returns before the older command sweep.

These consumer additions are prepared and unrun until the matching pair is
sealed; the assertions and commands above are not acceptance receipts.
