# Real-project corpus discovery

Updated: 2026-10-10 (Asia/Tokyo). Discovery is complete; semantic, compiler,
IDEA and performance acceptance are **pending**. See [fork-acceptance.md](fork-acceptance.md).

## Revisions and source coverage

Five official repositories were cloned outside ktlsp into
`C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010`. These are full working
trees with shallow history, not sparse subsets. All five checkouts were verified
clean against the pinned commits. Historical releases are deliberate: they retain
Java/Kotlin interoperability examples and stable build requirements. This does not
establish compatibility with their latest releases.

| Project / directory | Tag | Actual commit | Tracked Java / Kotlin files |
| --- | --- | --- | --- |
| [OkHttp](https://github.com/square/okhttp) / `okhttp-4.12.0` | `parent-4.12.0` | `4984568367caaf359b82c452bd28b5e192824d1c` | 152 / 251 |
| [Moshi](https://github.com/square/moshi) / `moshi-1.15.2` | `1.15.2` | `7f957b18ea123b7bb2ffc9d3c079e680eecf15e1` | 93 / 45 |
| [Retrofit](https://github.com/square/retrofit) / `retrofit-2.11.0` | `2.11.0` | `cc76c22a68e090f3dd898cbcb0bac30414f59c31` | 300 / 15 |
| [Spring Framework](https://github.com/spring-projects/spring-framework) / `spring-framework-6.2.0` | `v6.2.0` | `5024bb72279188f1d0dcfe19e8abdd4bdb9887c8` | 8,803 / 311 |
| [Gradle](https://github.com/gradle/gradle) / `gradle-8.14.3` | `v8.14.3` | `e5ee1df3d88b8ca3a8074787a94f373e3090e1db` | 11,143 / 1,203 |

Counts use `git ls-files '*.java' '*.kt'`, so they exclude untracked build outputs
and `.gradle` caches, but include checked-in tests, fixtures, templates and
documentation examples. `.kts` files are not counted as Kotlin source files.
They are not the resolved compilation-unit counts of a Gradle project model.
The external `discovery.json` records counts, module grouping, verified revisions,
clean status and SHA-256 hashes of the current 118 CEM main source files.

The following concrete source roots provide ordinary mixed production/application
code. Counts here are by the exact listed prefix, not by project-name grouping:

| Project / source root | Java / Kotlin | Meaning |
| --- | --- | --- |
| OkHttp `samples/guide/src/main/java/` | 31 / 19 | Java and Kotlin applications using the same Kotlin client API |
| OkHttp `okhttp/src/main/kotlin/` | 0 / 122 | Client implementation; Java consumers live in sample/test modules |
| Moshi `moshi/src/main/java/` | 31 / 2 | Java core plus Kotlin generic extensions in the Java source directory |
| Moshi `moshi-kotlin/src/main/java/` | 0 / 2 | Kotlin implementations of the Java adapter contract |
| Retrofit `retrofit/src/main/java/` | 54 / 1 | Java HTTP implementation and Kotlin suspend extensions call each other |
| Spring `spring-beans/src/main/` | 333 / 2 | Java bean contracts and Kotlin reified generic extensions |
| Spring `spring-core/src/main/` | 727 / 7 | Java core, coroutine bridges and Kotlin extensions |
| Gradle `platforms/ide/problems/src/main/` | 11 / 1 | Java constructs a Kotlin class implementing a Java interface |
| Gradle `platforms/core-configuration/kotlin-dsl/src/main/` | 0 / 180 | Kotlin extensions over Java Gradle APIs in other modules |

Moshi also declares a separate `java16` source set with one record-adapter Java
file. Retrofit declares `java14` and `java16` source sets with one Java file each.
These must not be merged blindly into the default main source set.

Local discovery did not provide five diverse independent mixed projects. Most
Minecraft repositories were Kotlin-only with Java dependencies, or Java-only.
`cloche` and `minecraft-codev` contain Java integration fixtures alongside Kotlin
plugins; ItemIndicator's Java files are Mixin code. Those do not replace the
selected ordinary interoperability corpus. mc-dev-lsp remains a semantic reference;
kross.nvim is a source/build-output bridge.

## CEM working tree and dependency evidence

Original root: `C:/Users/gummy/IdeaProjects/CreateEnchantableMachinery`.
HEAD: `09073e3d71c9f62a48009f3c3698bb6e159a0636`, branch
`mc1.21.1/neoforge/dev`. The working tree already contains user modifications;
the external manifest captures status and source hashes. It was inspected without
running Gradle or editing its files.

CEM has one Gradle root project, 25 Java files under `src/main/java` and 93 Kotlin
files under `src/main/kotlin`. All 25 Java files are in `cem/mixin`; two Kotlin
files are in `cem/mixinimpl`. Directory exclusion therefore leaves 91 Kotlin files
and zero project-owned Java files. Three extracted Create Java files under the
root `com/` directory are outside the configured source sets. The `bin/` directory
duplicates Kotlin source and is excluded from counts and acceptance.

The non-Mixin CEM slice primarily exercises Kotlin against Java dependencies.
Normal files can still contain Mixin-dependent statements: for example,
`util/BlockEntityExtension.kt` contains both the ordinary `holderLookup` extension
and casts to Mixin accessors. Acceptance excludes those accessor-dependent
statements as well as Mixin directories; the 91-file count alone does not prove
that every remaining file is independent of Mixin semantics.

The version catalog and wrapper declare Kotlin 2.4.0, JVM toolchain 21, Gradle
9.6.1, ModDevGradle 2.0.142, Minecraft 1.21.1, NeoForge 21.1.248,
Create 6.0.10-280, Flywheel 1.0.6, Ponder 1.0.87+mc1.21.1,
Registrate MC1.21-1.3.0+67 and KotlinForForge 5.12.0. Create is explicitly
non-transitive; Flywheel API and Curios API are compile-only dependencies.
`src/generated/resources` is added as resources, not as generated Java/Kotlin.

Existing artifacts were verified readable:

- `build/moddev/artifacts/neoforge-21.1.248-sources.jar` contains Minecraft
  `HolderGetter.java` and `BlockState.java`. `HolderGetter.getOrThrow(ResourceKey<T>)`
  returns `Holder.Reference<T>` in those sources.
- The Gradle cache contains binary and source JARs for Create 6.0.10-280 and
  Flywheel API 1.0.6, plus the Kotlin 2.4.0 compiler JAR.
- Create's cached sources define `DrillBlock extends DirectionalKineticBlock`,
  implementing `IBE<DrillBlockEntity>`, with `getRotationAxis(BlockState)` and
  `BlockEntityType<? extends DrillBlockEntity> getBlockEntityType()`.
- Existing `build/moddev/*LegacyClasspath.txt` files date from 2026-08-15;
  Minecraft artifacts date from 2026-08-12. These are runtime/bootstrap and cache
  evidence, **not a freshly resolved compileClasspath or a successful build**.

Subsequently, the current working-tree source was copied to the disposable
`cem-working-copy` directory alongside the five clones, excluding `.git`, IDE,
Gradle and build outputs. The original project remains untouched. Running
`gradlew.bat -p <copy> -I <ktlsp>/scripts/classpath-dump.init.gradle.kts ktlspDumpClasspath --offline -q --console=plain`
successfully resolved one module and 101 classpath entries, with no `UNRESOLVED`
records. One generated NeoForge JAR was initially absent despite configuration
resolution succeeding. `createMinecraftArtifacts --offline --console=plain`
then succeeded in 28 seconds using existing NFRT caches, creating the required
binary and sources in the copy. This establishes materialized dependency inputs;
it does not alone establish LSP semantic acceptance. A subsequent
`classes --offline --console=plain` succeeded in 47 seconds with Kotlin 2.4.0 and
JDK 21, preserving compiler warnings in `cem-classes-offline.log`. The real Neovim
`project` scenario also resolves the ordinary `EnchantableDrillBlock` superclass
to Create's `DrillBlock.java` and returns hover content before goto. Those controls
do not cover generic chains, valid override proposals, diagnostics or parity.

Logs are preserved under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010` as
`cem-classpath-offline.log`, `cem-tasks.log` and `cem-minecraft-artifacts.log`.

## Candidate semantic probes

Every row below is a source-derived expected case awaiting an independent compiler
check, concrete IDEA observation, and an asserted ktlsp result. None is a claimed
ktlsp failure or semantic pass. Paths are relative to the corresponding project.

| Case | File / token | Expected useful result | Status / next action |
| --- | --- | --- | --- |
| CEM valid Java override | `src/main/kotlin/io/github/cotrin8672/cem/content/block/drill/EnchantableDrillBlock.kt`, `getBlockEntityType` | Resolve the Create declaration, preserve the covariant block-entity type, no false override error | Pending: compile disposable source snapshot, check IDEA and editor |
| CEM generic dependency chain | `.../content/block/drill/EnchantableDrillBlockEntity.kt`, `holderLookup`, `getOrThrow`, `efficiency` | Infer enchantment holder members through the extension and Java generic API | Pending; avoid Mixin-dependent statements in the extension's file |
| CEM smart cast and nullable API | `.../content/block/drill/EnchantableDrillBlock.kt`, `blockEntity`, `placer`, `getEnchantments` | Refine the Kotlin interface after `is`; retain nullable `LivingEntity?` in valid override | Pending |
| CEM fluent generic API | `.../registry/BlockRegistration.kt`, `block`, `register`, `ENCHANTABLE_MECHANICAL_DRILL` | Resolve Registrate methods and infer `BlockEntry<EnchantableDrillBlock>` consistently | Pending |
| CEM extension receiver | `.../util/PoseStackExtension.kt` and `.../content/block/drill/EnchantableDrillRenderer.kt`, `use` | Select the local PoseStack extension; resolve receiver members inside its lambda | Pending |
| OkHttp ordinary Java override | `samples/guide/src/main/java/okhttp3/recipes/AsynchronousGet.java`, `Callback`, `onResponse`, `body` | Navigate to Kotlin contracts; complete valid callback overrides and Java-facing accessors | Pending |
| OkHttp generated Kotlin source | `okhttp/src/main/kotlin/okhttp3/internal/Util.kt`, `OkHttp.VERSION` | Resolve generated `OkHttp.kt` from the configured generated Java source directory | Pending: run `:okhttp:copyJavaTemplates` on clone before asserting generation |
| Moshi generic override | `moshi-kotlin/src/main/java/com/squareup/moshi/kotlin/reflect/KotlinJsonAdapter.kt`, `create`, `JsonAdapter.Factory` | Kotlin override resolves the Java factory method, with nullable adapter return and wildcard collection parameter | Pending |
| Moshi reified extension | `moshi/src/main/java/com/squareup/moshi/-MoshiKotlinExtensions.kt`, `adapter`, `nonNull` | Select the intended overload and preserve `JsonAdapter<T>` through Java/Kotlin calls | Pending |
| Moshi generated adapter | `kotlin/tests/codegen-only/src/test/kotlin/com/squareup/moshi/kotlin/codegen/ComplexGenericsInheritanceTest.kt` | Resolve KSP/KAPT-generated adapters and inherited generic types using the correct test source set | Pending: generate in clone using declared `kotlinTestMode` |
| Retrofit bidirectional suspend bridge | `retrofit/src/main/java/retrofit2/HttpServiceMethod.java`, `KotlinExtensions.awaitResponse`, and `KotlinExtensions.kt`, `Callback<T>` | Java facade resolves to Kotlin function; Kotlin callback resolves Java contracts and substituted types | Pending; use as first real corpus cross-language unsaved/rename probe |
| Retrofit nullable overloads | `retrofit/src/main/java/retrofit2/KotlinExtensions.kt`, `await`, `awaitNullable`, `awaitUnit` | Distinguish receiver nullability and JVM names; do not select an arbitrary overload | Pending |
| Spring generic extension | `spring-beans/src/main/kotlin/org/springframework/beans/factory/BeanFactoryExtensions.kt`, `getBeanProvider`, `getObject` | Resolve Java API; infer `ObjectProvider<T>` then `T` through reified extension | Pending |
| Spring strict nullability | `spring-core/src/main/kotlin/org/springframework/core/env/PropertyResolverExtensions.kt`, `getProperty` | Preserve nullable return without default and non-null return with default; honor strict JSR-305 settings | Pending |
| Gradle same-module bidirectional edge | `platforms/ide/problems/src/main/java/org/gradle/problems/internal/services/ProblemsBuildTreeServices.java`, `DefaultProblemsReportCreator`, and corresponding Kotlin implementation | Navigate Java construction to Kotlin class and Kotlin overrides to Java `ProblemReportCreator` | Pending; apply/inspect rename across both buffers, then compile copy |
| Gradle overloaded extension and receiver lambda | `platforms/core-configuration/kotlin-dsl/src/main/kotlin/org/gradle/kotlin/dsl/PropertyExtensions.kt`, `assign`, and `NamedDomainObjectProviderExtensions.kt`, `invoke` | Preserve provider variance, choose the compatible overload, infer lambda receiver `T` | Pending |

## Build prerequisites and generation

Discovery host has approximately 64 GiB RAM, 16 logical processors, and 585.5 GB
free on C: before clones. JDK 8, 11, 17, 21 and 22 are installed. JDK 14/16/23
were not found in the inspected locations. Android SDK platforms and build-tools
include version 34. Shell `python`/`python3` resolve to Windows app aliases and
the `py` launcher reports no installed Python; use the bundled runtime for harness
scripts. The default `bash` is WSL; use the recorded Git Bash executable.

| Project | Wrapper / Kotlin | Required model details and focused compiler tasks |
| --- | --- | --- |
| CEM | 9.6.1 / 2.4.0 | JDK 21; ModDevGradle-generated Minecraft/NeoForge artifacts, non-transitive Create plus explicit dependencies; compile only a disposable snapshot preserving dirty sources |
| OkHttp | 7.5 / 1.8.21 | Use JDK 17 for wrapper runtime; target Java 8; `:samples:guide:compileJava :samples:guide:compileKotlin`; `:okhttp:copyJavaTemplates` generates Kotlin `okhttp3/OkHttp.kt` under `build/generated/sources/java-templates/java/main`; Android test module is conditional |
| Moshi | 7.4.2 / 1.8.21 | JDK 17 toolchain; Java 8 baseline plus MR-JAR Java 16 compiled with JDK 17; `:moshi:compileJava :moshi:compileKotlin :moshi-kotlin:compileKotlin`; codegen test module defaults to KSP, supports `-PkotlinTestMode=KAPT` |
| Retrofit | 8.7 / 1.9.23 | Java 8 toolchain; Azul JDK 14/16 for MR-JAR tasks, with Foojay resolver; Android module is unconditional and requests SDK 34; `:retrofit:compileJava :retrofit:compileKotlin`; protobuf-generated sources in converter modules are separate |
| Spring | 8.10.2 / 1.9.25 | Main Java 17 and MR Java 21; Kotlin language/API 1.7, target 17, `-Xjsr305=strict`; `:spring-beans:compileJava :spring-beans:compileKotlin`; custom `optional` configuration contributes compile dependencies; repackaged sources/JARs and testFixtures need their own model |
| Gradle | 8.14.2 / 2.0.21 | **Build runtime must be JDK 17**, enforced by settings; included builds `build-logic`, `build-logic-commons`, `build-logic-settings`; `:problems:compileJava :problems:compileKotlin`; Kotlin DSL generates extensions from Java APIs; do not run the repository-wide `build` task |

Both Spring and Gradle need clone-local `core.longpaths=true` on this Windows
machine. Spring's initial checkout failed on a long test filename; the new clone
was restored from its pinned HEAD after enabling that setting. Gradle's long
resource paths required the same setting for accurate status traversal. No global
Git configuration was changed.

## Reproduction and next commands

Executed clone pattern (substitute the table's official URL, tag and directory):

```powershell
rtk git -c core.longpaths=true clone --depth 1 --single-branch --branch <tag> <url> C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/<directory>
rtk git -C C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/<directory> config core.longpaths true
rtk git -C C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/<directory> rev-parse HEAD
rtk git -C C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/<directory> status --short
rtk git -C C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/<directory> ls-files '*.java' '*.kt'
```

Next ordinary corpus editor smoke, from the ktlsp checkout after the baseline
release binary and Windows harness are ready:

```powershell
rtk proxy C:/Users/gummy/scoop/apps/git/current/bin/bash.exe dev/ktlsp-harness.sh java-project --root C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/okhttp-4.12.0 --file samples/guide/src/main/java/okhttp3/recipes/AsynchronousGet.java --token Callback
```

First compiler task group, from the Moshi clone with JDK 17 selected for the
process and isolated build/cache settings recorded in its run artifacts:

```powershell
rtk proxy ./gradlew.bat :moshi:compileJava :moshi:compileKotlin :moshi-kotlin:compileKotlin --no-daemon --console=plain
```

## Compiler and model execution

The following checks run against the complete pinned clone, using focused tasks;
they do not claim a repository-wide build or test suite. All non-CEM commands use
process-local `JAVA_HOME=C:/Program Files/Java/jdk-17`, `--no-daemon`,
`--max-workers=4`, and `--console=plain`. Network dependency downloads are allowed.
Build durations include dependency/build-logic work and concurrent builds; they
are compiler evidence, not LSP latency measurements.

| Project | Executed task group | Observed result | Evidence |
| --- | --- | --- | --- |
| CEM copy | `classes --offline` | Success, Kotlin 2.4.0 / Java 21; warnings retained | `cem-classes-offline.log` |
| OkHttp | `:samples:guide:compileJava :samples:guide:compileKotlin` | Success, 3m 34s; Kotlin core, TLS, mockwebserver and KAPT-generated guide sources compiled | `okhttp-compile.log` |
| Moshi | `:moshi:compileJava :moshi:compileKotlin :moshi-kotlin:compileKotlin` | Success, 2m 56s; main and Java 16 MR-JAR sources compiled | `moshi-compile.log` |
| Retrofit | `:retrofit:compileJava :retrofit:compileKotlin` | Success, 4m 49s; ordinary mutual Java/Kotlin core | `retrofit-compile.log` |
| Spring | `:spring-beans:compileJava :spring-beans:compileKotlin` | Success, 7m 8s; spring-core, repackaged libraries and spring-jcl dependencies compiled | `spring-compile.log` |
| Gradle | `:problems:compileJava :problems:compileKotlin` | Success, 28m 46s; included build logic and ordinary Java/Kotlin problem-reporting module compiled | `gradle-compile.log` |

Logs are under `C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010`.
Initial offline classpath dumps resolve 11 Moshi modules / 52 entries and 19 OkHttp
modules / 176 entries, but report 11 and 8 unresolved dependencies respectively in
other modules (codegen and optional sample/support modules). Subsequent online
dumps all exit successfully with zero unresolved dependencies:

| Project | Modules | Classpath entries | Distinct absent project outputs |
| --- | --- | --- | --- |
| Moshi | 11 | 68 | 1 |
| OkHttp | 19 | 202 | 4 |
| Retrofit | 26 | 201 | 6 |
| Spring | 26 | 1,090 | 29 |
| Gradle | 194 | 11,093 | 82 |

Evidence is in `<project>-classpath-online.log` under the log directory above.
The dumps use `scripts/classpath-dump.init.gradle.kts` and the same JDK/runtime
settings as the compiler tasks. Missing paths belong to unbuilt project JARs or
class directories, including modules outside the focused compiler task group.
Dependency resolution alone therefore does not prove complete-root model readiness;
missing project-output handling and source-set visibility remain to be verified.

Root reproduced a concrete remapped-module identity error against the real Gradle
dump. `:problems` has physical directory `platforms/ide/problems` and 82 classpath
entries. For its existing `DefaultProblemsReportCreator.kt`, the public
`daemon::module_path_for` returns `:platforms:ide:problems` instead of `:problems`.
The external Rust assertion fails with exit 1, while `classpath::parse_dump`
correctly retains `project_dir`. Evidence is under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-goal-20261010/module-mapping/`:
`baseline.rs`, `baseline.out`, and the UTF-8 copy `classpath.utf8.log` of the
completed dump. The first attempt to read PowerShell's UTF-16 log failed before
the assertion and is a probe setup error, not a product failure. Three callers
derive module-specific import/classpath or compiler work from this helper;
repair must consume Gradle's actual module-to-directory mapping. This reproduction
does not by itself attribute a separate navigation timeout to the mapping error.

The resolved project model, module visibility and generated source roots remain
prerequisites for semantic acceptance. Entire root indexing and latency must be
measured separately from focused submodule probes. Keep complete roots in the
performance corpus and record source-set scope; a fast result from a small module
is not a whole-project result.

## Exact read-only corpus probes

`dev/ktlsp-harness.sh corpus --root <project> --file <manifest.json>` runs the same
assertions through ktlsp or the official IntelliJ plugin. `dev/nvim_lsp_probe.lua`
shares engine configuration, import checks, file identity, UTF-16 anchor positions
and completion matching with the compiled `mixed` fixture. The corpus scenario
opens query source files only; expected declaration files are read as oracle data,
not opened to make missing index entries available. It requires exactly one
definition with the expected identifier range. It records first/last responses,
indexing retries, source hashes, revision/status and graceful shutdown. These
read-only probes do not prove unsaved editing, references, rename or latency on
the real roots.

Generated per-root manifests are outside the checkout at
`C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/probes/<directory>.json`.
The 13 source/declaration anchor pairs were validated for uniqueness and UTF-16
positions against the pinned sources. A portable minimal manifest is:

```json
{
  "schema_version": 1,
  "project_id": "moshi-1.15.2",
  "build_tool": "gradle",
  "probes": [{
    "id": "moshi-jsonreader-member",
    "source": "moshi-kotlin/src/main/java/com/squareup/moshi/kotlin/reflect/KotlinJsonAdapter.kt",
    "anchor": "reader.beginObject()",
    "token": "beginObject",
    "target": "moshi/src/main/java/com/squareup/moshi/JsonReader.java",
    "target_anchor": "public abstract void beginObject()",
    "target_token": "beginObject"
  }]
}
```

Source/target paths are relative to the selected root. `completion: false` disables
completion for the two interface-only navigation controls below. `target_token`
can differ from `token`, as required by Kotlin's `@JvmName("awaitNullable")`.
The following table preserves the portable candidate data; line numbers are
one-based, while JSON/LSP positions are zero-based UTF-16 columns. Directory and
file prefixes are the source roots documented above.

| Root / source file | Query anchor and token | Exact declaration | Intended semantic coverage |
| --- | --- | --- | --- |
| CEM `.../drill/EnchantableDrillBlockEntity.kt:34` | `return super.getBreakSpeed()` / `getBreakSpeed` | Create `com/simibubi/create/content/kinetics/base/BlockBreakingKineticBlockEntity.java:157`, `protected float getBreakSpeed()` | Inherited protected Java member through `DrillBlockEntity` |
| CEM same file `:32` | `holderLookup(Registries.ENCHANTMENT).getOrThrow(Enchantments.EFFICIENCY)` / `getOrThrow` | NeoForge `net/minecraft/core/HolderGetter.java:10`, `default Holder.Reference<T> getOrThrow(ResourceKey<T> resourceKey)` | Kotlin extension result plus Java generic/overload selection; excludes the `TagKey<T>` overload |
| OkHttp `samples/guide/src/main/java/okhttp3/recipes/AsynchronousGet.java:35` | `client.newCall(request).enqueue(` / `enqueue` | `okhttp/src/main/kotlin/okhttp3/Call.kt:68`, `fun enqueue(responseCallback: Callback)` | Java chained receiver to Kotlin interface member |
| OkHttp same file/anchor | `newCall` | `okhttp/src/main/kotlin/okhttp3/OkHttpClient.kt:268`, `override fun newCall(request: Request): Call` | Java receiver to concrete Kotlin override |
| Moshi `moshi-kotlin/src/main/java/com/squareup/moshi/kotlin/reflect/KotlinJsonAdapter.kt:189` | `: JsonAdapter.Factory` / `Factory` (goto only) | `moshi/src/main/java/com/squareup/moshi/JsonAdapter.java:319`, `public interface Factory {` | Nested Java interface implemented by Kotlin |
| Moshi same file `:69` | `reader.beginObject()` / `beginObject` | `moshi/src/main/java/com/squareup/moshi/JsonReader.java:328`, `public abstract void beginObject()` | Kotlin calls Java abstract contract |
| Retrofit `retrofit/src/main/java/retrofit2/HttpServiceMethod.java:205` | `KotlinExtensions.awaitResponse(call, continuation)` / `awaitResponse` | `retrofit/src/main/java/retrofit2/KotlinExtensions.kt:92`, `suspend fun <T> Call<T>.awaitResponse()` | Java facade to Kotlin suspend extension |
| Retrofit same Java file `:250` | `KotlinExtensions.awaitNullable(call, continuation)` / `awaitNullable` | Same Kotlin file `:65`, `suspend fun <T : Any> Call<T?>.await(): T?`, token `await` | JVM name maps specifically to nullable Kotlin overload |
| Retrofit same Kotlin file `:39` | `= response.body()` / `body` | `retrofit/src/main/java/retrofit2/Response.java:151`, `public @Nullable T body()` | Kotlin calls Java nullable generic member; anchor starts after the local variable with the same name |
| Spring `spring-beans/src/main/kotlin/org/springframework/beans/factory/BeanFactoryExtensions.kt:30` | `getBeanProvider<T>().getObject()` / `getObject` | `spring-beans/src/main/java/org/springframework/beans/factory/ObjectProvider.java:57`, `default T getObject()` | Reified Kotlin extension chain to zero-argument Java overload |
| Spring same Kotlin file `:64` | `getBeanProvider(ResolvableType.forType(` / `getBeanProvider` | Same Java directory `BeanFactory.java:257`, `<T> ObjectProvider<T> getBeanProvider(ResolvableType requiredType)` | Select `ResolvableType`, excluding `Class<T>` and no-argument Kotlin extension |
| Gradle `platforms/ide/problems/src/main/java/org/gradle/problems/internal/services/ProblemsBuildTreeServices.java:120` | `new DefaultProblemsReportCreator(` / `DefaultProblemsReportCreator` | `platforms/ide/problems/src/main/kotlin/org/gradle/problems/internal/impl/DefaultProblemsReportCreator.kt:49`, `class DefaultProblemsReportCreator(` | Java constructs Kotlin class in `:problems` |
| Gradle same Kotlin file `:56` | `) : ProblemReportCreator {` / `ProblemReportCreator` (goto only) | `platforms/ide/problems-api/src/main/java/org/gradle/api/problems/internal/ProblemReportCreator.java:26`, `public interface ProblemReportCreator {` | Kotlin implements another Gradle module's Java contract |

CEM's declaration oracles are extracted outside the indexed root from the verified
Create 6.0.10-280 and NeoForge 21.1.248 source archives. Its manifest uses
`target_file` for that oracle text and `target_uri_suffix` for the complete qualified
source path. A dependency may be returned as an archive URI or an extracted file;
both its qualified path and exact identifier range must match. These oracles are
never opened as editor buffers. Preserve or reconstruct them from the recorded
archive versions before using the manifest on another machine.

Example command after selecting an immutable release binary:

```powershell
rtk proxy C:/Users/gummy/scoop/apps/git/current/bin/bash.exe dev/ktlsp-harness.sh corpus --root C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/moshi-1.15.2 --file C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/probes/moshi-1.15.2.json --bin <release>/ktlsp.exe --no-build
```

For IntelliJ, set `KTLSP_PROBE_SERVER=intellij`, `IJLS_PLUGIN`, `IJLS_SERVER`
and `IJLS_JDK`, with isolated XDG directories and server home. The scenario requires
successful Gradle import; initialization/capabilities alone do not pass. The current
comparison explicitly reuses `C:/Users/gummy/.gradle` through `GRADLE_USER_HOME`
for already-compiled dependency/build-tool inputs and records that path. CEM uses
JDK 21; the other roots use JDK 17. Sources remain unchanged, although build-tool
import can write ignored model/build caches.

The first ktlsp Moshi run passes 7/7 checks, including the three semantic checks.
The first interface definition needs one indexing retry, which is retained in
`corpus-requests.jsonl`; it is not a timing sample. Evidence is under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-probes-20261010/moshi-ktlsp-1`.
The independent IntelliJ Moshi run passes its three semantic checks after a
successful Gradle import, with one response per request. Its artifact directory is
`moshi-intellij-1` beside the ktlsp run above. The actual Gradle module `:problems`
is physically under `platforms/ide/problems`; URI navigation from a global source
index alone will not prove that the correct module classpath was selected. Module
visibility, generated outputs, diagnostics, invalid/ambiguous code and real-root
editing/rename/performance remain acceptance gaps.

### Startup observations and indexed controls

`KTLSP_CORPUS_WAIT_INDEX=1` requires both project and dependency indexing log
notifications, using the same readiness meaning as `dev/bench_initialize.py`.
`KTLSP_CORPUS_READY_TIMEOUT_MS` changes the default 600-second readiness timeout.
Timeout leaves semantic checks unmeasured. Failed indexing or missing dependency
sources remain independent failed coverage checks. Once both phases finish, the
selected semantic checks still run and label their phase
`first-response-after-indexing-with-incomplete-inputs` when applicable. A subset
passing under incomplete inputs does not pass whole-project acceptance.

Indexed and IntelliJ requests each use exactly one response after readiness;
startup retries are retained separately and do not count as freshness or timing
evidence. Definition positions remain at the identifier start. Subsequent
completion controls use the identifier end, or an explicit `completion_prefix`
such as `"newC"`. This distinguishes a filtered member query from an unfiltered
first page with `isIncomplete: true`; all request positions and raw results are
retained. This control adjustment does not replace testing actual incomplete
unsaved buffers.

The initial IntelliJ runs use identifier-start completion positions and have
these limited results, under `ktlsp-corpus-probes-20261010/<root>-intellij-1`:

| Root | Observed semantic checks | Limitation / next action |
| --- | --- | --- |
| Moshi | 3/3 pass | Initial directory is `moshi-intellij-1`; complete-root coverage remains open |
| Retrofit | 6/6 pass | Confirms facade, nullable `@JvmName` overload and Java generic `body()` targets |
| Spring | 4/4 pass | Confirms the zero-argument `getObject` and `ResolvableType` provider overload |
| OkHttp | 0/4 match configured expectations | `newCall` completion is an incomplete first page of 25 accessors; definition returns compiled `OkHttpClient.class` rather than source. Recheck filtered completion and source attachment; do not infer a missing method from this page |
| CEM | 3/4 match configured expectations | Both exact archive definitions and `getOrThrow` completion pass. The unfiltered getter completion includes synthetic property `breakSpeed`, not `getBreakSpeed`; verify full prefix and compiler-backed property access separately |
| Gradle | Unmeasured | Import fails because its build requires JDK 17 while the importer selects JDK 22 despite `JAVA_HOME` and `defaultSdk` 17. Fix the temporary importer JVM setup without changing tracked build files |

Initial cold ktlsp queries on the other roots did not wait for library indexing;
empty results and timeouts from those runs are startup observations. The first
indexed series completes with zero failed dependency workers but missing-source
counts of Moshi 4, OkHttp 13, Retrofit 30, Spring 27, Gradle 145 and CEM 6. Its first
five roots stopped at the coverage check before a policy refinement; they do not
contain indexed semantic measurements. CEM's indexed run continues and passes the
exact inherited `getBreakSpeed` definition, while both completion checks and the
`HolderGetter.getOrThrow` definition fail. A uniform series with filtered
completion positions is the next comparison. The initial Retrofit `body` probe
accidentally selected the local variable of the same name; the manifest anchor
was corrected to `= response.body()` before the independent IntelliJ pass.

### Compiler-plugin inventory and pending editing cases

This inventory comes from the pinned build files and generated outputs. A plugin
being present or a compiler task succeeding does not establish LSP support.

| Root | Actual configuration / evidence | Useful next probe |
| --- | --- | --- |
| CEM | Kotlin JVM and ModDev; no KAPT, KSP, all-open, no-arg or serialization plugin in the current build | Generated NeoForge binary/source availability is a build-output case, not evidence of a CEM compiler transform |
| OkHttp | `samples/guide/build.gradle` applies `kotlin-kapt`; `kapt deps.moshiKotlin` selects Moshi codegen. The focused guide build creates `build/generated/source/kapt/main/okhttp3/recipes/kt/ParseResponseWithMoshi_GistJsonAdapter.kt` and `..._GistFileJsonAdapter.kt` | Find the generated adapter and its `fromJson`/`toJson` members from an unsaved source use, using the configured generated source root without opening the target |
| Moshi | `kotlin/tests/codegen-only/build.gradle.kts` defaults `kotlinTestMode` to KSP, with selectable KAPT. Kotlin 1.8.21 / KSP 1.8.21-1.0.11 are pinned; `GeneratedAdaptersTest.JsonAnnotation` is annotated with `@JsonClass(generateAdapter = true)` | First run the focused KSP test generation/compiler task; main-module compilation did not create these test adapters. Then verify the actual generated class name, source root and member targets |
| Retrofit | `retrofit-response-type-keeper` applies KAPT with AutoService and Incap. `RetrofitResponseTypeKeepProcessor` writes `META-INF/proguard/retrofit-response-type-keeper-<type>.pro`; AutoService supplies service registration | Resource generation and processor discovery are relevant controls; do not invent generated members for this processor |
| Spring | `spring-web`, `spring-webflux` and `spring-messaging` apply `kotlinx-serialization`. No all-open application was found in this pin. `KotlinSerializationJsonHttpMessageConverterTests` contains `@Serializable SerializableBean` and unannotated `NotSerializableBean` | Compare generated `serializer()` completion with the unannotated negative control. Its declaration is synthetic; inspect IDEA navigation before fixing an expected source range |
| Gradle | Build logic registers `org.gradle.api.HasImplicitReceiver` for `kotlin-sam-with-receiver`; Java `Action<T>` has that annotation. Kotlin DSL compiler setup also installs the assignment plugin | Verify implicit receiver members in an annotated Java SAM lambda and reject the same receiver in an unannotated control. Assignment behavior is a separate semantic transform |

[KSP generates new code and cannot modify existing source](https://kotlinlang.org/docs/ksp-overview.html).
[SAM-with-receiver changes how the first Java SAM parameter is exposed to a Kotlin lambda](https://kotlinlang.org/docs/sam-with-receiver-plugin.html).
[All-open changes class and member openness, including meta-annotations and annotated superclasses](https://kotlinlang.org/docs/all-open-plugin.html).
[No-arg generates a synthetic constructor callable through reflection, not direct Java/Kotlin calls](https://kotlinlang.org/docs/no-arg-plugin.html).
These transformations require distinct positive and negative expectations. In
particular, completion must not advertise `new Foo()` merely because no-arg is
enabled. Separate disposable fixtures are required for transformations absent
from the selected real roots.

The proposed incomplete-code slice preserves a compiled disk golden, applies
buffer-only edits, then checks useful expected members and excluded unrelated
members for Java and Kotlin: a partial member, a dangling dot, a syntax error in
an unrelated peer and an unknown receiver alongside a known typed receiver.
Outgoing `didChange` versions and first responses after a delivery barrier must
be recorded, then buffers restored and source hashes checked. These cases are
pending; their RPC correctness phase must remain separate from later actual
Neovim popup-display measurements.
