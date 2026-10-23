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
are recorded checkpoints, not a claim that later in-progress API changes have
already been rebuilt or that the public API is frozen. Existing manifests/probe
bytes and their denominators stay unchanged. Nominal identities do not establish generated enum/record members or
object-owned callable lowering. Nullable, generic and value-class ABI shapes
remain unsupported. Production Mixin integration, runtime/remapping, MixinExtras,
Lapis and IDEA parity remain unproved.
