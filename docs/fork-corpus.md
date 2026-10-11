# Real-project corpus discovery

Current interpretation (2026-10-12): historical harness exit codes and source-coverage
FAIL labels below include operational checks. They are not project analysis scores.
The corpus report now separates matched/mismatched semantic responses, unanswered or
unstarted requests, execution state and source-input metadata. A deadline leaves a case
unmeasured; unavailable sources matter to correctness when a required case cannot resolve.
See checkpoint 34 in [fork-acceptance.md](fork-acceptance.md) for the verified classification.

Latest verified slice (2026-10-12): Gradle's warnings-as-errors and large project-model
transport defects are repaired. The unchanged real-project manifests match all six
incomplete and three golden requests on their first valid responses. Edit delivery
and restoration complete; ten failed and twenty-five missing source inputs remain
metadata. Checkpoint 35 in [fork-acceptance.md](fork-acceptance.md) links commands,
source/binary hashes and native reports. This is focused analysis evidence; matched
IDEA latency/memory and broader editing acceptance remain open.

Updated: 2026-10-12 (Asia/Tokyo). Discovery is complete; semantic, compiler,
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

Portable per-root manifests are in [dev/corpus](../dev/corpus/README.md), with
source-snapshot hashes and CEM archive-extraction instructions. The original runs
use equivalent external manifests at
`C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/probes/<directory>.json`.
All six portable manifests were checked against the actual roots: 13 unique
source/declaration anchor pairs, matching text hashes and 24 semantic checks.
A minimal manifest is:

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
rtk proxy C:/Users/gummy/scoop/apps/git/current/bin/bash.exe dev/ktlsp-harness.sh corpus --root C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-20261010/moshi-1.15.2 --file dev/corpus/moshi-1.15.2.json --bin <release>/ktlsp.exe --no-build
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
`HolderGetter.getOrThrow` definition fail. The next series below applies filtered
completion positions uniformly. The initial Retrofit `body` probe
accidentally selected the local variable of the same name; the manifest anchor
was corrected to `= response.body()` before the independent IntelliJ pass.

### Uniform post-index first responses

The second series uses the same complete roots and 13 anchor pairs, with
completion at the full identifier's end in both engines. ktlsp uses the immutable
third release, SHA-256
`72a900ebe45b4a04daa027d656cd5d7af14c0d9096f5ccaa600a574d344ea7f0`.
All six roots finish both index phases, then each semantic request consumes one
response. Zero dependency workers fail, but missing sources fail the separate
coverage check in every root. The 9/24 matching selected semantic checks do not
pass root acceptance.

| Root | Project / library files reported | Missing sources | ktlsp selected checks | Actual remaining result |
| --- | --- | --- | --- | --- |
| Moshi | 150 / 25,123 | 4 | 1/3 match | `Factory` returns seven unrelated nested declarations; `beginObject` returns both project and dependency Moshi 1.12.0 declarations |
| OkHttp | 404 / 59,235 | 13 | 3/4 match | `newCall` returns both `Call.Factory` and concrete `OkHttpClient` declarations; both completions and `enqueue` definition match |
| Retrofit | 313 / 56,641 | 30 | 1/6 match | Facade navigation also returns dependency 2.9.0; `awaitNullable` is empty; `body` returns field/parameter and method declarations from both versions. Only `body` completion matches |
| Spring | 9,080 / 76,698 | 25 | 1/4 match | Generic-chain `getObject` completion/navigation is empty; `getBeanProvider(ResolvableType)` navigates to the Kotlin no-argument extension instead of the Java overload |
| Gradle | 13,820 / 94,203 | 146 | 2/3 match | Java-to-Kotlin constructor completion/navigation matches; Java interface navigation returns both project source and a local-JAR stub |
| CEM | 122 / 37,825 | 6 | 1/4 match | Inherited `getBreakSpeed` definition matches; its completion and generic `HolderGetter` completion/navigation do not |

Evidence: `ktlsp-corpus-probes-20261010/<root>-ktlsp-indexed-2/artifacts/`
contains `corpus-results.json` and raw `corpus-requests.jsonl`. These file counts
are the server's actual entire-root index reports, not claims that every indexed
file belongs to the correct compilation source set. Spring's missing-source count
changes from 27 to 25 and Gradle's from 145 to 146 between series, reflecting
external input availability; retain each run's counts rather than attributing
that change to the unchanged binary.

The independent filtered IntelliJ series uses official plugin revision
`9cfad18005470e4187090013e08f92ec9e437219` and ILS-263.6379.0. Successful Gradle
import precedes every measured root. It matches 17/21 measured checks; Gradle's
three checks remain unmeasured after import failure.

| Root | Selected checks matching the same assertions | Limitation |
| --- | --- | --- |
| Moshi | 3/3 | Exact project-owned Java declarations selected |
| Retrofit | 6/6 | Confirms facade, JVM-name nullable overload and method rather than local/field `body` |
| Spring | 4/4 | Confirms generic chain and Java `ResolvableType` overload |
| OkHttp | 1/4 | Filtered `newCall` completion now matches. `enqueue` completion is an empty complete list and definition is empty; `newCall` navigates to one compiled `OkHttpClient.class` declaration rather than the configured source target. This requires importer/source-attachment investigation |
| CEM | 3/4 | Both exact dependency definitions and `getOrThrow` completion match. Getter completion has synthetic property `breakSpeed`, with `isIncomplete: true`; the configured method-name assertion fails. This does not establish that later completion pages lack that method |
| Gradle | Unmeasured | The importer still runs JDK 22 although the temporary launch sets JDK 17 in `JAVA_HOME`, `PATH`, `defaultSdk`, and a task-local `GRADLE_USER_HOME/gradle.properties` with `org.gradle.java.home`. The build's JDK-17 guard fails; no source/build guard was changed or ignored |

Evidence: `<root>-intellij-filtered-2/artifacts/` in the same external directory.
All source hashes and tracked status controls pass in both series. The earlier
identifier-start results remain separate. IDEA's getter-property representation
and compiled-class navigation need explicit compiler-backed controls; the exact
matcher has not been relaxed to turn them into source matches. Raw editor logs
are private; inspection copies redact application credential values and retain
source/copy hashes and replacement counts. The harness removes credential-named
variables from its own process before invoking the official plugin, preserving
the required toolchain environment.

A focused fourth-release Moshi rerun, SHA-256
`8b9166c05d7769e8a3cecbfe18dcfad89acc7a0bf9fb96d51b925b08981a8c0c`,
changes `Factory` to the correct project and dependency declarations instead of
the seven unrelated targets. Both definition checks still fail because each
returns the extra Moshi 1.12.0 target; completion matches, with four missing source
inputs retained as a coverage failure. Evidence:
`moshi-1.15.2-ktlsp-indexed-fourth-1/artifacts/`. Module/version visibility and
project-versus-binary identity remain required shared-layer work.

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
[No-arg adds a zero-argument constructor](https://kotlinlang.org/docs/no-arg-plugin.html).
Its documented reflection-only access requires a pinned compiler check: root's
1.9.23 output has a public, non-synthetic constructor callable by a separate
Java 17 compiler client. Java/Kotlin and source/binary expectations must be
checked separately; see [the compiler controls](fork-acceptance.md).
These transformations require distinct positive and negative expectations.
Separate disposable fixtures are required for transformations absent from the
selected real roots.

### Unsaved incomplete-code fixture control

[mixed-incomplete.json](../dev/corpus/mixed-incomplete.json) uses the existing
`mixed_project.lua` golden and the same `corpus` route. Ten buffer-edit cases cover
Java and Kotlin partial members, dangling dots, an unrelated broken peer, and
known/unknown receivers. Each negative unknown receiver also queries a useful
known receiver in the same edited state and excludes wrong typed members. This
makes twelve completion requests, with no acceptance retries. An LSP null negative
response is valid; a timeout or protocol error is not.

The golden compiles in an external copy with Maven 3.10.0, Kotlin 2.0.21 and JDK 21
using cached dependencies. Evidence is
`ktlsp-corpus-probes-20261010/incomplete-compiler-original/maven-cached.log`.
An initial offline attempt selected the wrong isolated Maven repository and
failed to resolve the compiler plugin; `maven.log` retains that setup failure.
Neither compiler invocation saves editor buffers or modifies the golden project.

The fourth ktlsp release and ILS-263.6379.0 each pass 72/72 checks, comprising ten
edit/restore cycles, twelve first-response completion requests and the associated
delivery, exclusion and disk controls. Both runs record twenty outbound `didChange`
versions and twenty matching barriers. Evidence is
`incomplete-ktlsp-2/artifacts/` and `incomplete-intellij-2/artifacts/` under the same
external directory as the real-root probes. The IntelliJ control requires actual
Maven import success. The initial ktlsp run's four negative assertions incorrectly
treated valid LSP null responses as unavailable; its raw `incomplete-ktlsp-1`
artifact is retained and is not a fabricated-member product failure.

The transport barrier is Neovim's existing all-buffer change flush, followed by
outbound version checks and one response per query. Buffers are restored after
each case, with unchanged disk hashes and clean-buffer assertions. The ktlsp
fixture reports one background-indexed Kotlin file; the two Java/Kotlin query
buffers are attached explicitly. This does not prove complete Maven source-root
discovery. These small fixture results do not establish incomplete-code acceptance
on the six real roots, normal debounce freshness, or popup-display latency.
The real-root observations below use the same fields and retain failing controls.

### Real-root unsaved incomplete-code comparison

The six `dev/corpus/*-incomplete.json` manifests each contain five cases: compiled
disk golden, partial member, dangling dot, broken peer and paired known/unknown
receivers. Each root makes six completion requests because the unknown case also
queries its known control. This is **30 cases and 36 requests per engine**. A
negative unknown response is accepted only when the same-state known receiver is
useful; an empty response alone cannot pass that case.

The ktlsp binary is the immutable fourth release, SHA-256
`8b9166c05d7769e8a3cecbfe18dcfad89acc7a0bf9fb96d51b925b08981a8c0c`.
The independent oracle is ILS-263.6379.0 through the pinned official plugin.
Every measured request uses its first response, with `attempts: 1`; no retry is
used to accept a case. Counts below distinguish whole cases from requests whose
predicate and required known control both pass.

| Root | ktlsp cases / 5 | IDEA cases / 5 | ktlsp qualified requests / 6 | IDEA qualified requests / 6 |
| --- | ---: | ---: | ---: | ---: |
| Moshi | 5 | 5 | 6 | 6 |
| OkHttp | 5 | 0 | 6 | 0 |
| Retrofit | 0 | 5 | 0 | 6 |
| Spring | 0 | 4 | 0 | 5 |
| Gradle | 5 | 5 | 6 | 6 |
| CEM non-Mixin call | 0 | 5 | 0 | 6 |

These are 15/30 cases and 18/36 qualified requests for ktlsp, and 24/30 cases and
29/36 qualified requests for IDEA. Retrofit's Kotlin facade, Spring's generic
provider chain and CEM's `holderLookup().getOrThrow` are already unavailable in
ktlsp's disk-golden controls. Their later failures do not isolate an incomplete
parser regression. IDEA's OkHttp `Call.enqueue` candidate is absent in the disk
golden as well; full-root import succeeds, so the limitation of that imported
model remains explicit instead of weakening the expected compiled API.

Spring's IDEA dangling-dot request passed in `incomplete-1` and failed in
`inc-ctx2`, where `getObject` was absent and `getBeanProvider` incorrectly appeared.
Both responses are retained. The latter run's paired unknown control passes after
preserving the extension receiver scope. This is a variable first-response
observation; outbound notification order does not establish completion of server
recomputation. Neither a stable recovery result nor a specific cause is claimed.
The original unknown edit had incorrectly placed the known call outside an
expression-bodied extension; that manifest error and the later long-cache-path
RocksDB initialization failure are preserved separately from product failures.
Retrofit and CEM's final unknown edits also preserve their original return or
initializer statements.

Each engine records 48 delivered edit/restore versions and barriers, with every
delivery, restored buffer and disk-hash control passing. The five Git clones also
retain their tracked status. CEM's disposable snapshot has no `.git`, so that
status control is unavailable; its queried and edited peer texts are hash-checked.
No test saves an editor buffer or modifies the original CEM project.

ktlsp still fails the independent source-input coverage check for every root:

| Root | Background project files | Library files | Missing-source count |
| --- | ---: | ---: | ---: |
| Moshi | 149 | 25,123 | 4 |
| OkHttp | 403 | 59,235 | 13 |
| Retrofit | 313 | 56,644 | 29 |
| Spring | 9,079 | 76,698 | 25 |
| Gradle | 13,820 | 95,859 | 147 |
| CEM | 121 | 37,825 | 6 |

Both indexing phases finish with zero failed dependencies. Query and edited peer
buffers are attached before indexing; the peer is attached because it is edited,
rather than to expose an oracle declaration. Background counts therefore differ
from the earlier read-only probes. Shared dependency availability also changed
during independent imports: Retrofit's prior count was 30. These input changes
are recorded and are not attributed to a binary fix.

Moshi's four warnings have been individually classified from cached artifacts:
Kotlin BOM 1.8.21 and 1.9.22 and coroutines BOM 1.8.0 have POM packaging and no JAR;
`listenablefuture:9999.0-empty-to-avoid-conflict-with-guava` has a JAR with zero
class entries. The current coverage check still fails; sound classification of
source-less artifacts and coverage of actual semantic inputs remain follow-up
work. This classification does not turn unrelated missing source archives into
passing coverage.

Evidence is under `ktlsp-corpus-probes-20261010/`, with an aggregate
`real-incomplete-matrix.json`. The final per-root series are:

- ktlsp: Moshi/OkHttp/Gradle `incomplete-fourth-1`, Spring
  `incomplete-spring-context-2`, Retrofit/CEM `inc-ctx2`.
- IDEA: Moshi/OkHttp `incomplete-1`, Spring/Retrofit/CEM `inc-ctx2`, Gradle `inc-1`.

Each run contains `artifacts/corpus-results.json` and
`artifacts/corpus-requests.jsonl`. Gradle's IDEA run reuses only the completed,
isolated oracle cache after verifying its old backend exited; preceding backend
logs were snapshotted. It is a warm correctness control. These selected RPC
results do not establish whole-root editing coverage, normal debounce freshness,
popup-display latency or a performance budget.

### Gradle IDEA import JVM and primary-constructor ranges

Full Gradle-root import now succeeds with JDK 17. The earlier failed imports
remain recorded. `JAVA_HOME`, `defaultSdk`, task-local `org.gradle.java.home` and
the disposable clone's ignored `.idea/gradle.xml` alone had left the backend
selecting JDK 22. Inspection of the installed
`com.jetbrains.ls.imports.gradle.GradleToolingApiHelper` shows that
`findTheMostCompatibleJdk` first reads this JVM system property:

```powershell
$env:IJ_JAVA_OPTIONS = '"-Dcom.jetbrains.ls.imports.gradle.java.home=C:/Program Files/Java/jdk-17"'
```

Apply this option only to the pinned Gradle-root oracle. It is an internal
implementation detail of ILS-263.6379.0, not a promised public configuration ABI.
The pinned official plugin preserves it when adding the isolated server home.
Without the property, the inspected method sorts compatible installed JDKs by
version before its `JAVA_HOME` fallback. No tracked settings, daemon criteria or
JDK-version guard were changed, and the guard's bypass property was not enabled.

The installed executable SHA-256 is
`9a732f33aa4ae59a2112249734a710ea6a93c7df284a4561a0bc5e5920ab96dc`;
`plugins/kotlin.lsp/lib/modules/language-server.workspace-import.jar` is
`b4fc5f0e11f798fe0cbe73481b19512ccc7b65f8e1a126db221629ca1366410b`.
Evidence includes `GradleToolingApiHelper.javap.txt` and
`gradle-jvm-property-evidence.json`. A task-local observational init script then
records actual settings evaluation for the root and all three included builds:
all four use PID 50276, JDK 17.0.7 and
`ignore_build_java_version_check: false`. The callback only records these facts;
it changes neither JVM selection nor the guard. Records are in
`gradle-jvm-observations/*.json` under the same external evidence directory.
The task-local Gradle home keeps cache/wrapper junctions; the real home remains
unchanged.

IDEA's Java `new DefaultProblemsReportCreator(...)` definition names the correct
source file but selects its exact primary-constructor parameter list:
zero-based UTF-16 range `48:34` to `55:1`. The identifier is `48:6` to `48:34`.
Both are valid navigation extents for the compiled six-argument constructor,
whose JVM signature is independently retained in
`gradle-primary-constructor.javap.txt`. The portable Gradle manifest alone adds
the reviewed constructor range through optional `target_ranges`; it retains the
identifier alternative and requires the pinned target text hash and in-file
bounds. Wrong files, other ranges, enclosing ranges and extra locations fail.
`dev/corpus_matcher_test.lua` passes 18 pure checks, including these rejection
controls and UTF-16 bounds. Other manifests' definition expectations are unchanged.

The original `gradle-jvm-property-4` run retains its identifier-range assertion
failure. With the explicit alternative, `gradle-8.14.3-intellij-range-1` passes
all three first-response semantic requests and all ten Lua checks, including
import success, graceful shutdown and unchanged source/Git controls. Its Bash
wrapper subsequently fails with `project: unbound variable` at line 608; the
harness script was edited while Bash awaited Neovim. The current script has no
such access at that line, consistent with the known hazard of changing a running
Bash script's unread tail. The queried source files remain unchanged. The routed
run's exit remains 1; semantic checks passing does not relabel its whole-harness
result as passing. Separate stable-wrapper controls now pass the basic scenario
with exit 0 and complete the portable compiler-plugin corpus/native-UI route
without an undefined variable. That latter route retains exit 1 for its three
literal getter-label failures while its native UI stage passes; its harness hash
is unchanged before/after. Evidence is `fifth-fixed/basic/artifacts` under the
goal evidence root and `pp-idea-2/artifacts` under this corpus evidence root.
Neither control relabels the original Gradle wrapper failure.

The same reviewed manifest on the fourth ktlsp binary passes constructor completion
and identifier navigation, while interface navigation still returns both the
project declaration and its own local-JAR stub. Its missing-source count is 138,
with 13,820 project and 95,272 library files. This later dependency availability
differs from the incomplete-code run and is not a semantic fix or timing result.
All owned oracle backends were verified exited after these runs.

### Fifth release scope control and Gradle 7 regression

The immutable fifth release (`c3f4f63b35ec8ff0a4df3cd41aa168cd58f670fe6faff384d5780a705d441076`)
passes all three exact Gradle-root golden requests. In particular, Kotlin's
`ProblemReportCreator` navigation returns only its project declaration, rather
than the project declaration and local-JAR stub returned by the fourth release.
The independent source-input check still fails (27 missing archives), so the
complete routed run retains exit 1. The initial `indexed 0 project files` message
precedes installation of the resolved model and does not count its later source
scan; that startup message alone is not a final project-coverage measurement.

The same fifth binary regresses Moshi: zero of three golden requests and zero
of five incomplete-code cases pass. All six incomplete requests use attempt 1;
the unknown-receiver response excludes the named members but its same-phase
known control is empty, so it supplies no passing coverage. Eight delivered
edit/restore controls and all source/Git preservation checks pass. No expectations
were relaxed. Moshi's independent source-input check also retains its four
previously classified missing archives.

An independent invocation of the exact embedded init script on Moshi's pinned
Gradle 7.4.2 fails with native exit 1: `BuildIdentifier.buildPath` is unavailable
at both project-identity emission sites. No resolved-model cache is published;
the request trace reports `query-scope-incomplete`. This is a model-extraction
regression, not evidence that an empty semantic result is correct. Compatibility
must preserve included-build identities rather than flatten every old build to
the root. The exact script, failure log and exit record are preserved under
`ktlsp-goal-20261010/fifth-gradle-compat/`; the first PowerShell collection attempt
stopped on native stderr and is separate from the recorded native exit.

Runs are `moshi-1.15.2-ktlsp-fifth-golden-1`,
`gradle-8.14.3-ktlsp-fifth-golden-1` and
`moshi-1.15.2-ktlsp-fifth-incomplete-1` under the corpus evidence root. Each has
an unchanged before/after harness SHA-256
`505cddf47352e94f7bfab7624fac345a3ed0111ced8232c12331074a5fe23f36`.
All three routed exits remain 1. These correctness runs replace neither the
earlier six-root comparison nor performance acceptance.

### Sixth immutable release: all six roots

Binary SHA-256 is
`fa0148d65badc1cb8e95683a5d7bb76a742795a75532afed07e6567be2bfdf90`.
All twelve golden/incomplete runs completed with matching binary, shared harness,
manifest, queried-source and target hashes. Every one of the 60 requests received
a response on attempt 1. All 48 outbound edit/restore barriers pass, and the five
Git checkouts retain their original status. CEM's disposable copy has no `.git`;
its actual source hashes supply the preservation control.

| Project | Golden requests | Incomplete requests | Incomplete cases | Missing source archives |
| --- | --- | --- | --- | --- |
| CEM, ordinary code | 0/4 | 0/6 | 0/5 | 3 |
| Moshi | 3/3 | 6/6 | 5/5 | 1 |
| OkHttp | 3/4 | 6/6 | 5/5 | 0 |
| Retrofit | 3/6 | 0/6 | 0/5 | 4 |
| Spring Framework | 1/4 | 0/6 | 0/5 | 16 |
| Gradle | 3/3 | 6/6 | 5/5 | 27 |

This is 13/24 golden requests (6/13 complete cases) and 18/36 incomplete
requests (15/30 cases). Negative completion alone earns no case pass: the paired
known receiver must also return useful members. All six persisted model-v3
snapshots have complete source sets and zero unresolved classpath entries, and
both indexing notifications arrive in all twelve runs. Missing source archives
remain a separate failing input-coverage assertion for five projects; successful
model extraction does not make those archives available.

Moshi's prior Gradle 7.4.2 regression is repaired, and Gradle's single exact
project target remains correct. CEM's four golden responses are all empty.
Retrofit still lacks useful JVM-facade and `@JvmName` completion. Spring's
reified provider chain is empty, while an overloaded Java provider call resolves
to a Kotlin extension instead of the Java declaration independently selected by
IDEA. OkHttp's `newCall` returns two source locations, the interface and concrete
implementation. Its unchanged one-target assertion fails. Earlier IDEA runs
return one compiled implementation target and also fail the exact-source-path
assertion; multiplicity and source navigation are recorded separately from
proving a wholly unrelated symbol.

The aggregate, guards and gap interpretation are
`sixth-corpus-observations.json`, `sixth-corpus-guards.json` and
`sixth-gap-interpretation.json` under the corpus evidence root. Individual runs
use `<project>-ktlsp-sixth-{golden,incomplete}-1`, with raw results, request traces,
model snapshots and before/after source snapshots. These runs establish remaining
correctness gaps and do not supply latency or IDEA-parity acceptance.

### Eighth immutable release: CEM source and incomplete-code repair

Binary SHA-256 is
`20454030ec7356e8c11e1733d7a62fbd2f0bf0dfeb6fb92b0188f0903ea370e7`.
The two CEM manifests and all queried/edited/oracle sources remain unchanged.
Every request receives its first response, all eight edit/restore barriers pass,
and both servers shut down cleanly. The local NeoForge JAR now contributes
5,865 source files with 103,029 symbols instead of 5,865 class-name stubs.

| CEM probe | Sixth release | Eighth release |
| --- | --- | --- |
| Generic holder completion and exact source definition | 0/2 | 2/2 |
| Superclass completion and exact source definition | 0/2 | 0/2 |
| Incomplete requests, including paired known receiver | 0/6 | 6/6 |
| Complete incomplete-code cases | 0/5 | 5/5 |
| Missing dependency source archives | 3 | 3 |

`super.getBreakSpeed` still reports `unknown-receiver-type`: the actual class
extends Java `DrillBlockEntity` and delegates `EnchantableBlockEntity`, leaving
multiple direct supertypes in the current super inference. Its failure is
preserved. The independent input-coverage assertion still fails on three missing
archives, so both routed runs retain exit 1. Other five roots were not rerun for
this focused checkpoint; no aggregate parity or performance result is inferred.

Evidence is `eighth-cem-observations.json`, `eighth-corpus-controller.json`,
and `cem-working-copy-ktlsp-eighth-{golden,incomplete}-1/artifacts/` under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-probes-20261010/`.
Before/after controller guards cover the immutable binary, harness/helpers,
manifests and every queried, edited peer and expected declaration source.

### Ninth immutable release: CEM direct-supertype repair

The input-matched ninth release is SHA-256
`e4b726b8920be45ec06941c71c1fc5955bc327c393487ad9d3098b738e7e8ad1`,
source revision `2b2325e`. The unchanged golden manifest now passes all four
requests, including the exact Create superclass definition; all six incomplete
requests and five cases also pass on their first response. Both indexing phases,
dirty-buffer restoration, graceful shutdown and every source/helper/binary hash
guard pass.

Both complete runs still exit 1 for dependency coverage: four failed archives
and three missing sources. The audit distinguishes three readable module-only
compatibility source archives from one actual non-UTF8 Commons Compress source
entry. Guava's empty compatibility binary needs no definitions. KFF has no
top-level classes but contains eight nested JARs; each matches a separately
selected binary in both main and test by SHA. Product completeness still treats
that archive conservatively. Patchy's two-class binary still needs source or
verified binary facts. These observations do not relabel the strict coverage
assertion as passing.

The later Commons Compress encoding audit follows the exact parent POM's javac
declaration (`commons-parent:66`, ISO-8859-1), with 404/404 CRC and round-trip
checks. Actual javac succeeds for the declared encoding and for normalized
UTF-8 with identical class bytes, while raw UTF-8 fails. Two other source files
have UTF-8 comment spellings; the policy is a declaration, not per-entry guessing.
No product conversion has shipped. Evidence is
`ktlsp-source-encoding-20261011/{archive-audit,javac-controls,other-archive-classification}.json`.

Evidence is `cem-working-copy-ktlsp-ninth2-{golden,incomplete}-1/` under this
corpus root and `ktlsp-goal-20261010/source-archives-cem/ninth-cem-and-archives.json`.
The saved model has complete classpaths and four complete configured compiler
tasks; output freshness is a separate requirement. The other five projects need
current immutable rechecks, and no performance or aggregate IDEA-parity claim
follows from this focused CEM result.

### Tenth immutable release: CEM and Retrofit

The input-matched tenth release has SHA-256
`342dafb060cb7912b58b32a105bb3016f1ca64ae759ec8e926b87e8f95c01ee8`.
All 270 guarded product inputs match before and after its 998-test Rust run and
release build. The existing corpus manifests and semantic expectations are
unchanged. These are concurrent correctness runs, with no latency acceptance.

| Project | Golden requests | Incomplete requests | Incomplete cases | Failed / missing dependency sources | Routed exits |
| --- | --- | --- | --- | --- | --- |
| CEM, ordinary code | 4/4 | 6/6 | 5/5 | 1 / 5 | 1 / 1 |
| Retrofit 2.11.0 | 6/6 | 6/6 | 5/5 | 1 / 5 | 1 / 1 |

Every semantic request matches on its first response after both indexing phases.
Incomplete-code cases include a useful known receiver paired with the expected
empty unknown receiver; passing the negative alone earns no case pass. All
delivered edit/restore controls, source/helper/binary guards and clean shutdown
checks pass. The independent source-coverage assertion remains the only failure
in each whole run: CEM golden 8/9 and incomplete 34/35; Retrofit golden 12/13 and
incomplete 35/36.

CEM retains 5,865 NeoForge source files with 103,029 symbols. Commons Compress's
non-UTF8 source remains the one failed archive. Missing inputs are Patchy,
Kotlin stdlib jdk7/jdk8 compatibility modules, coroutines jdk8 and KFF's outer
nested archive. Readable empty compatibility source archives now succeed as IO,
but module-bearing binaries still require definition-coverage evidence. The
selected empty Guava compatibility binary now proves its empty surface. The
strict missing-input count therefore changes independently of semantic results.

Retrofit now resolves the Kotlin suspend facade and exported `@JvmName` names
from Java while keeping Kotlin source names intact. Its failed archive is
`stax:stax:1.2.0`, whose `com/wutka/dtd/Scanner.java` fails strict UTF-8 decoding.
The five missing inputs are kotlin-stdlib-jdk7/jdk8 1.8.21, stax-api 1.0.1,
xmlParserAPIs 2.6.2 and xpp3 1.1.3.3. Model extraction and completed indexing do
not make these source artifacts complete.

Evidence is under `C:/Users/gummy/AppData/Local/Temp/ktlsp-corpus-probes-20261010/`:
`tenth-corpus-controller.json`,
`cem-working-copy-ktlsp-tenth-{golden,incomplete}-1/artifacts/`,
`retrofit-tenth-verified-report.json` and its referenced before/after guards.
Retrofit's guarded final golden run is `tenth-golden-2`; the original golden-1
controller assertion/interruption and launcher transition are retained separately.
The corrected external launcher accepts the accurate
`first-response-after-indexing-with-incomplete-inputs` phase only with independent
both-index and first-attempt controls. No product probe or expectation was weakened.

Moshi, OkHttp, Spring Framework and Gradle still need current immutable replays.
The broad tenth Java scenario also exposes an inherited-override incoming-call
regression; its repair must pass a fresh full/release and editor checkpoint.

### Tenth hierarchy/source-refresh repair: current editor evidence

The subsequent immutable `tenth-fixed-2` release has SHA-256
`f42d3c6d60fd62e1f4c86916bc7f331bdbb16896acb812cb6e9721300908d331`.
All 1,019 Rust tests pass, with three ignored; the same 270 inputs match before
and after tests and release compilation. Its guarded CEM/Retrofit replays retain
the exact existing manifests, incomplete edits and expected targets:

| Project | Golden requests | Incomplete requests | Whole run checks | Failed / missing sources | Routed exits |
| --- | --- | --- | --- | --- | --- |
| CEM, ordinary code | 4/4 | 6/6 | 8/9 and 34/35 | 1 / 5 | 1 / 1 |
| Retrofit 2.11.0 | 6/6 | 6/6 | 12/13 and 35/36 | 1 / 5 | 1 / 1 |

All 22 semantic requests match on the first response, with both indexing phases
complete. Every edit/restore delivery, useful known receiver control, graceful
shutdown and source/manifest/helper/binary guard passes. Each routed run retains
the independent coverage failure. Evidence is
`ktlsp-corpus-probes-20261010/tenth2-verified-replay-report.json` and the fresh
`{cem-working-copy,retrofit-2.11.0}-ktlsp-tenth2-{golden,incomplete}-1/artifacts/`.
CEM retains 34,783 library files, including 5,865 NeoForge source files and no
NeoForge stubs; Retrofit retains 26,405 library files. Source encoding, missing
binary-only/nested inputs and configured-producer versus output freshness limits
remain unchanged.

The separate Java harness now passes 174/179 checks: the incoming-call regression
is fixed and all seventeen complete override rename controls pass. Five previous
diagnostic failures remain. Actual applied Java/Kotlin rename snapshots compile
and preserve the expected bytecode targets; they are distinct from a nonempty
edit-list assertion. Library lifecycle passes 13/13 and the portable JVM facade
passes 81/81 plus nine compiler/runtime controls. See
[fork-acceptance.md](fork-acceptance.md) for exact evidence and remaining rename
limits. These concurrent runs still provide no latency acceptance or IDEA parity.

An initial external controller stalled after its Moshi editor process completed;
that raw attempt is preserved as incomplete controller evidence. The subsequent
fresh four-root audit is recorded below. No semantic expectation or shared harness
was changed to address the wrapper.
The final precommit `tenth-fixed-3` receipt repeats the full Rust/release checks
after removing only the fixture generator's trailing blank line. Its generated
four-file fixture and both release binaries are byte-identical to fixed-2.

### Tenth-fixed-2: remaining root audit and Gradle allocation failure

The six completed Moshi, OkHttp and Spring replays pass **29/29** semantic requests
on attempt 1, with no retries or expectation changes. Both indexing phases,
buffer-only edits, exact outbound versions, restoration and source guards pass.
Together with CEM and Retrofit, this is **51/51 measured requests across five real
roots**; it does not cover the sixth root or establish complete analysis coverage.

| Project | Golden requests | Incomplete requests | Whole run checks | Failed / missing sources | Routed exits |
| --- | --- | --- | --- | --- | --- |
| Moshi 1.15.2 | 3/3 | 6/6 | 8/9 and 35/36 | 0 / 2 | 1 / 1 |
| OkHttp 4.12.0 | 4/4 | 6/6 | 9/10 and 35/36 | 0 / 4 | 1 / 1 |
| Spring Framework 6.2.0 | 4/4 | 6/6 | 9/10 and 35/36 | 4 / 14 | 1 / 1 |
| Gradle 8.14.3 | NOT_MEASURED | NOT_MEASURED | Golden 5/7; incomplete absent | Library readiness failed / incomplete unknown | 1 / unknown |

The original v2 external controller incorrectly required every request's phase
to equal the run's indexing phase. Its three incomplete-run phase guards remain
false in the original evidence. A separate audit derives the expected phase from
each unchanged manifest's `buffer_edits`: one disk control follows indexing and
five requests follow unsaved buffer delivery. It verifies attempt 1 and exact
outbound versions, without requerying the server or changing expectations.

The Gradle golden log contains `memory allocation of 1290240 bytes failed` after
a file-watch rejection reporting insufficient memory. It announces 11,325 project
files but no library completion and executes zero required semantic requests.
The incomplete run has no result or request artifact after controller interruption;
its server exit and readiness outcome cannot be inferred. Neither run supplies a
semantic pass. Allocation-time server peak and system commit were not captured in
that original run; late free-memory observations do not identify its cause. The
isolated monitored follow-up is recorded below, with no performance claim.

Evidence is `fourroots-tenth2-v2-verified-report.json`, its concise
`fourroots-tenth2-v2-summary.json`, and the referenced raw artifacts under the
corpus evidence root. All 26 input hashes match in the captured after-guard at
`2026-10-10T22:20:00.931933Z`, before the shared harness was released for the
separate bare-call fixture. The original missing controller-final receipt remains
missing. Source coverage, compiler-output freshness and current IDEA/performance
acceptance remain open.

### Isolated Gradle follow-up: excessive client memory during watch registration

The unchanged `tenth-fixed-2` binary was replayed alone through the existing real
Neovim corpus route, with process and system memory sampling. The measured client
is Neovim 0.12.5 x64 running **ktlsp**, not IDEA LSP. The root agent forcibly
stopped its verified owned process tree to release host memory; this run is
**HOST-RESOURCE-PRESSURE-ABORT**, not a natural allocation failure or an acceptance
pass. All eleven input hashes match before and after the run.

| Measurement | Observed value |
| --- | --- |
| Neovim process lifetime peak commit | 63,475,847,168 bytes |
| Neovim maximum sampled private bytes | 62,120,669,184 bytes |
| Neovim process lifetime peak working set | 53,535,596,544 bytes |
| ktlsp process lifetime peak commit | 5,982,179,328 bytes |
| ktlsp process lifetime peak working set | 3,630,755,840 bytes |
| Maximum sampled system commit / limit | 95,120,838,656 / 96,102,813,696 bytes |
| Minimum sampled system commit headroom | 90,001,408 bytes |
| Project / library files / library symbols | 11,325 / 60,759 / 1,245,495 |
| Watch registration | 8,796 patterns; acknowledgement about 88 seconds later |

The earlier observations of approximately 40 GB for Neovim and 1.1 GB for ktlsp
were intermediate samples, not lifetime peaks. Forty-seven samples were collected
approximately five seconds apart. Commit, physical working set and system totals
are different measurements; the boot-wide system commit peak is not used as a run
peak.

All 8,796 patterns describe paths inside this workspace: 4,869 source directories,
3,925 output directories and two configuration patterns. Neovim groups these
absolute string globs under the same workspace base and folds their LPEG patterns
into one union before starting the native recursive watcher. Pattern count does
not establish the number of kernel watch handles. The code path and registration
delay are verified; individual allocation call stacks were not captured.

After the delay, indexing completed and three golden requests matched on their
first response. These are separate observations from the preceding zero-query
failure; they do not change its result. The monitored run still has three failed
and thirty missing dependency sources, no final `corpus-results.json`, and forced
termination. Its printed shutdown assertion does not prove clean process exit.

Evidence under the corpus root is
`gradle-tenth2-oom-monitored-1-terminal-summary.json`, the referenced memory
JSON/JSONL, and
`gradle-8.14.3-ktlsp-tenth2-oom-monitored-1/{artifacts,xdg-state/nvim-data/lsp.log}`.
The watch/model audit is under
`C:/Users/gummy/AppData/Local/Temp/ktlsp-watch-registration-20261011/`.

### Bounded watch repair: same large Gradle workload

The immutable `eleventh-fixed-2` release, SHA-256
`08fea5789ef522d00b0f4b012febeb555de249c92f8e883df1d268c464d9eab7`, was replayed
through the same existing corpus route. All twelve guarded inputs match. It indexes
the same 11,325 project files, 60,759 library files and 1,245,495 library symbols.
The model registration contains one pattern; both initial and model registrations
are acknowledged in the same log second. One-second log resolution does not imply
exact zero latency.

| Measurement | Before repair | After repair |
| --- | --- | --- |
| Model watch patterns | 8,796 | 1 |
| Neovim lifetime peak commit | 63,475,847,168 bytes | 9,261,056 bytes |
| Neovim lifetime peak working set | 53,535,596,544 bytes | 20,205,568 bytes |
| ktlsp lifetime peak commit | 5,982,179,328 bytes | 5,969,633,280 bytes |
| ktlsp lifetime peak working set | 3,630,755,840 bytes | 4,217,020,416 bytes |
| Maximum sampled system commit | 95,120,838,656 bytes | 38,921,887,744 bytes |
| Minimum sampled commit headroom | 90,001,408 bytes | 34,317,193,216 bytes |
| Minimum sampled physical memory available | 1,409,024 bytes | 46,680,809,472 bytes |
| Failed / missing dependency inputs | 3 / 30 | 3 / 25 |

The new run collects 98 samples at 0.5-second intervals, with a 750-second duration
bound and resource ceilings of 1 GiB Neovim private bytes, 8 GiB server private
bytes, at least 8 GiB system commit headroom and 4 GiB physical memory available.
No ceiling fires. These ceilings protect the test host; they are not accepted UX
budgets. The old run uses a different sampling interval and product revision, so
this is a diagnostic comparison rather than a controlled statistical benchmark.
Neither run measures a defined post-index steady-state window.

All three golden requests match exactly on their first response. Both stdout and
`corpus-results.json` report **9 PASS, 1 FAIL**; the sole failed check is dependency
source completeness. The module-only classification repair accounts for five fewer
missing sources. Server memory remains a separate optimization task.

The raw monitor status is `controller-ended-with-live-owned-abort`: it observes the
controller ending while the retained server handle is still live. Independent
handle auditing finds **zero TerminateProcess calls**, no resource-cap reason, all
owned processes exited 125 ms later, and every retained handle closed. Preserve
that raw status separately from the actual exit evidence. Controller, Neovim and
server native exit codes are one. The first shutdown receives a successful null
response, but the client treats `is_stopped()` (also true while stopping) as process
exit and reaches VimLeavePre too soon. That handler sends a second shutdown and
exit; the second shutdown receives `-32600 Invalid request`. Four tiny independent
protocol controls confirm normal shutdown exits zero and duplicate shutdown is
rejected. The corpus assertion is not clean-server-exit proof. The
preexisting Gradle daemon remains alive with the same creation identity and is
protected throughout.

Evidence under the same corpus root is
`gradle-watch-cap-eleventh2-1-terminal-summary.json`, SHA-256
`1fb5c3199adead18450ebb9e14ba9f3ed4098f236c4b65e085de07f5dd0a12b1`,
its referenced raw memory JSON/JSONL and logs, and
`gradle-8.14.3-ktlsp-watch-cap-eleventh2-1/{artifacts,xdg-state/nvim-data/lsp.log}`.

### Twelfth release: CEM native completion UI and lifecycle

The immutable twelfth-fixed-2 binary, SHA-256
`7c17b5f0991774b37c7cee3bf73936de426f2476f51ada8e3e984a970670dce0`,
and eleventh-fixed-2 were compared through the same guarded fixed-2 harness.
Both pass the four unchanged CEM completion/exact-navigation assertions. Both
index 34,783 library files and 825,029 symbols, with one failed and two missing
source inputs; coverage remains **FAIL**.

The new native UI probe removes the selector from the unsaved
`EnchantableDrillBlockEntity.kt` return line, enters insert mode and types the dot
after `super`. Both versions make one first completion request and receive zero
items, with the semantic reason `unknown-receiver-type`. No useful menu appears;
zero successful first/warm samples means no completion performance result. The
existing fully named selector still resolves to the exact Java declaration.
The multiple-supertype receiver path is a hypothesis to reduce and verify against
compiler/IDEA behavior; neither arbitrary-super selection nor relaxed matching is
an acceptable repair.

Both RPC and UI phases now independently record one acknowledged shutdown, one
exit notification, actual server exit/signal zero and no forced termination.
Source/binary/fixture guards remain unchanged. Each overall CEM harness exits one
because coverage and useful UI completion fail; clean shutdown does not override
those failures. Evidence:
`ktlsp-completion-measurement-20261011/run-{baseline,candidate}-2-cem/`.
The mixed fixture's 62 useful native UI samples per version and their p50/p95 are
recorded in [fork-acceptance.md](fork-acceptance.md); those timings do not stand in
for the missing CEM samples or for the other five real roots.
Source/output/configuration/external-path boundaries also have six focused tests;
notification-classification cost, server allocation peaks and remaining dependency
coverage require further work.
