#!/usr/bin/env bash
# Scriptable editor harness for ktlsp. Runs headless Neovim against the real ktlsp binary and
# stores logs, traces, temp projects, and cache state in one run directory.
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: dev/ktlsp-harness.sh <scenario> [options]

scenarios:
  basic          generated two-file project; local + cross-file goto
  features       dev/sample feature smoke; refs, completion, auto-import, edits
  java           generated Java/Kotlin project; Java features and unsaved member completion
  mixed          generated Maven project; compiled mixed-language edits, goto, refs, and rename
  corpus         existing project comparison; requires --root and --file JSON probe manifest
  library        compiled Gradle project; stdlib/JDK sources and unsaved dependency edits
  semantic       committed semantic probe; chains, receivers, and KMP narrowing
  project        existing project health check; requires --root and --file; optionally probes implementation
  java-project   existing Java project goto/references probe; requires --root, --file, and --token
  emacs-project  existing project health/perf probe through batch Emacs + Eglot
  gradle-live    dev/gradle-sample live probe + classpath configuration-cache probe
  gradle-compile alias for gradle-live
  comprehensive  dev/gradle-sample broad library/project probe

options:
  --root DIR       project root for scenarios that accept one
  --file FILE      Kotlin/Java file for project probes, or JSON manifest for corpus
  --token TEXT     symbol to use for the project implementation or java-project goto probe
  --target TEXT    expected implementation target URI substring for the project probe
  --occurrence N   textual occurrence to use for a token probe (default: 2)
  --definition-token TEXT       symbol to use for the project goto-definition probe
  --definition-target TEXT      expected definition target URI substring
  --definition-occurrence N     textual occurrence for the definition probe (default: 1)
  --needle TEXT    symbol/needle for the Emacs documentHighlight probe
  --burst N        semanticTokens/full burst size for the Emacs probe
  --bin FILE       ktlsp binary to run
  --release        build/use target/release instead of target/debug
  --no-build       skip cargo build
  --run-dir DIR    explicit artifact directory
  -h, --help       show this help

mixed/corpus environment:
  KTLSP_MIXED_MVN          Maven executable (otherwise mvn/mvn.cmd on PATH)
  KTLSP_MIXED_MAVEN_REPO   Maven cache (default: artifacts/m2)
  KTLSP_MIXED_SERVER       ktlsp (default) or intellij
  KTLSP_UI_SAMPLES         mixed builtin completion cycles per language (default: 1)
  KTLSP_UI_CASES           optional corpus popup/accepted-text manifest; uses the same UI probe
  KTLSP_PROBE_SERVER       corpus backend: ktlsp (default) or intellij
  KTLSP_CORPUS_SHUTDOWN_OBSERVE_MS  optional corpus exit-observation wait (default: 15000)
  IJLS_PLUGIN/IJLS_SERVER  intellij-lsp.nvim plugin and server paths for intellij
  IJLS_JDK/JAVA_HOME       JDK for the IntelliJ server

gradle-live environment:
  KTLSP_GRADLE_RELOAD=1    disposable offline dependency/generated-source reload probe
  KTLSP_PROCESSOR_RELOAD=1 real KAPT/KSP option changes on one client (JDK 17)
  GRADLE_USER_HOME        existing Gradle wrapper/plugin cache for that probe
USAGE
}

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
scenario="${1:-basic}"
if [ "$scenario" = "-h" ] || [ "$scenario" = "--help" ]; then
  usage
  exit 0
fi
if [ $# -gt 0 ]; then
  shift
fi

root_arg=""
file_arg=""
token_arg=""
target_arg=""
occurrence_arg="2"
definition_token_arg=""
definition_target_arg=""
definition_occurrence_arg="1"
needle_arg="${KTLSP_HARNESS_NEEDLE:-}"
burst_arg="${KTLSP_HARNESS_BURST:-}"
bin_arg="${KTLSP_BIN:-}"
profile="debug"
build=1
run_dir=""

while [ $# -gt 0 ]; do
  case "$1" in
    --root)
      root_arg="${2:?missing value for --root}"
      shift 2
      ;;
    --file)
      file_arg="${2:?missing value for --file}"
      shift 2
      ;;
    --token)
      token_arg="${2:?missing value for --token}"
      shift 2
      ;;
    --target)
      target_arg="${2:?missing value for --target}"
      shift 2
      ;;
    --occurrence)
      occurrence_arg="${2:?missing value for --occurrence}"
      shift 2
      ;;
    --definition-token)
      definition_token_arg="${2:?missing value for --definition-token}"
      shift 2
      ;;
    --definition-target)
      definition_target_arg="${2:?missing value for --definition-target}"
      shift 2
      ;;
    --definition-occurrence)
      definition_occurrence_arg="${2:?missing value for --definition-occurrence}"
      shift 2
      ;;
    --needle)
      needle_arg="${2:?missing value for --needle}"
      shift 2
      ;;
    --burst)
      burst_arg="${2:?missing value for --burst}"
      shift 2
      ;;
    --bin)
      bin_arg="${2:?missing value for --bin}"
      shift 2
      ;;
    --release)
      profile="release"
      shift
      ;;
    --no-build)
      build=0
      shift
      ;;
    --run-dir)
      run_dir="${2:?missing value for --run-dir}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ "$scenario" == emacs-* ]]; then
  if ! command -v emacs >/dev/null 2>&1; then
    echo "emacs is required on PATH" >&2
    exit 2
  fi
else
  if ! command -v nvim >/dev/null 2>&1; then
    echo "nvim is required on PATH" >&2
    exit 2
  fi
fi

if [ -z "$run_dir" ]; then
  run_base="${KTLSP_HARNESS_BASE:-/tmp/ktlsp-harness}"
  run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$-${scenario}"
  run_dir="$run_base/$run_id"
fi

artifacts="$run_dir/artifacts"
projects="$run_dir/projects"
mkdir -p "$artifacts" "$projects" "$run_dir/cache" "$run_dir/xdg-state"

if [ -z "$bin_arg" ]; then
  if [ "$build" -eq 1 ]; then
    if [ "$profile" = "release" ]; then
      cargo build --release --bin ktlsp --bin bench
    else
      cargo build --bin ktlsp --bin bench
    fi
  fi
  bin_arg="$repo/target/$profile/ktlsp"
else
  if [ "$build" -eq 1 ]; then
    if [ "$profile" = "release" ]; then
      cargo build --release --bin bench
    else
      cargo build --bin bench
    fi
  fi
fi

if [ -f "$bin_arg.exe" ]; then
  bin_arg="$bin_arg.exe"
fi

bench="$repo/target/$profile/bench"
if [ "$profile" = "debug" ] && [ ! -x "$bench" ]; then
  bench="$repo/target/debug/bench"
fi

export KTLSP_BIN="$bin_arg"
export KTLSP_CACHE_DIR="$run_dir/cache"
export KTLSP_TRACE="$artifacts/trace-events.jsonl"
export RUST_LOG="${RUST_LOG:-ktlsp=debug}"
export XDG_STATE_HOME="$run_dir/xdg-state"

run_status=0

write_basic_project() {
  local project="$projects/basic"
  mkdir -p "$project"
  cat > "$project/Main.kt" <<'KOTLIN'
package demo

fun helper(): Int = 42

fun main() {
    val g = Greeter("world")
    println(g.greet())
    val potato=""
    println(helper())
}
KOTLIN
  cat > "$project/Greeter.kt" <<'KOTLIN'
package demo

class Greeter(val name: String) {
    fun greet(): String = "Hello, " + name
    fun potato() = 3
}
KOTLIN
  printf '%s\n' "$project"
}

write_mixed_project() {
  local project="$projects/mixed"
  nvim -l "$repo/dev/mixed_project.lua" "$project" || return $?
  printf '%s\n' "$project"
}

write_library_project() {
  local project="$projects/library"
  mkdir -p "$project/gradle/wrapper" "$project/src/main/kotlin" "$project/src/main/java/app"
  for file in gradlew gradlew.bat gradle/wrapper/gradle-wrapper.jar; do
    cp "$repo/dev/gradle-wrapper-template/$file" "$project/$file"
  done
  chmod +x "$project/gradlew"
  cat > "$project/gradle/wrapper/gradle-wrapper.properties" <<'PROPERTIES'
distributionBase=GRADLE_USER_HOME
distributionPath=wrapper/dists
distributionUrl=https\://services.gradle.org/distributions/gradle-9.6.1-bin.zip
networkTimeout=10000
validateDistributionUrl=true
zipStoreBase=GRADLE_USER_HOME
zipStorePath=wrapper/dists
PROPERTIES
  cat > "$project/settings.gradle" <<'GRADLE'
pluginManagement { repositories { gradlePluginPortal(); mavenCentral() } }
rootProject.name = 'ktlsp-library-fixture'
GRADLE
  cat > "$project/gradle.properties" <<'PROPERTIES'
org.gradle.parallel=false
org.gradle.workers.max=2
kotlin.compiler.execution.strategy=in-process
kotlin.stdlib.default.dependency=false
PROPERTIES
  cat > "$project/gradle/libs.versions.toml" <<'TOML'
[libraries]
stdlib = "org.jetbrains.kotlin:kotlin-stdlib:2.2.20"
TOML
  cat > "$project/build.gradle" <<'GRADLE'
plugins { id 'org.jetbrains.kotlin.jvm' version '2.4.0' }
repositories { mavenCentral() }
java { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
kotlin { compilerOptions { jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17 } }
// This ordinary fixture has no scripts or compiler transformations.
tasks.withType(org.jetbrains.kotlin.gradle.tasks.KotlinCompile).configureEach { pluginClasspath.setFrom([]) }
configurations {
    librarySources { canBeConsumed = false; canBeResolved = true; transitive = false }
}
dependencies {
    implementation libs.stdlib
    implementation('org.seleniumhq.selenium:selenium-api:2.42.2') { transitive = false }
    librarySources 'org.jetbrains.kotlin:kotlin-stdlib:2.2.20:sources'
    librarySources 'org.seleniumhq.selenium:selenium-api:2.42.2:sources'
}
def librarySourceFiles = configurations.librarySources.incoming.files
tasks.register('prepareLibrarySources') {
    inputs.files(librarySourceFiles)
    doLast { inputs.files.files.each { println('LIBRARY_SOURCES\t' + it.absolutePath) } }
}
GRADLE
  cat > "$project/src/main/kotlin/Main.kt" <<'KOTLIN'
package app

import java.sql.Connection

fun main() {
    val xs = listOf(1, 2, 3)
    println(xs)
}

fun useConnection(connection: Connection) {
    connection.close()
}
KOTLIN
  cp "$repo/dev/corpus/fixtures/java-diagnostics/"*.java "$project/src/main/java/app/"
  cp "$repo/dev/corpus/fixtures/library/Windows1252MetadataUse.java" "$project/src/main/java/app/"
  printf '%s\n' "$project"
}

run_library_compile() {
  (cd "$1" && ./gradlew classes prepareLibrarySources --console=plain --no-daemon)
}

write_classpath_config_cache_project() {
  local project="$projects/classpath-config-cache"
  mkdir -p "$project/app" "$project/gradle/wrapper"
  cp "$repo/dev/gradle-wrapper-template/gradlew" "$project/gradlew"
  cp "$repo/dev/gradle-wrapper-template/gradlew.bat" "$project/gradlew.bat"
  cp "$repo/dev/gradle-wrapper-template/gradle/wrapper/gradle-wrapper.jar" \
    "$project/gradle/wrapper/gradle-wrapper.jar"
  cp "$repo/dev/gradle-wrapper-template/gradle/wrapper/gradle-wrapper.properties" \
    "$project/gradle/wrapper/gradle-wrapper.properties"
  chmod +x "$project/gradlew"
  cat > "$project/settings.gradle.kts" <<'KOTLIN'
rootProject.name = "ktlsp-classpath-config-cache"
include(":app")
KOTLIN
  cat > "$project/app/build.gradle.kts" <<'KOTLIN'
repositories {
    mavenCentral()
}

// The init script registers its observer before this callback. The classpath dump task must still
// appear when a plugin creates compileClasspath this late in the project lifecycle.
afterEvaluate {
    pluginManager.apply("java")
    dependencies.add("implementation", "com.example:ktlsp-harness-missing:0.0.0")
}
KOTLIN
  cat > "$project/build.gradle.kts" <<'KOTLIN'
import java.io.File
import java.io.IOException
import java.net.URLClassLoader
import java.util.jar.JarEntry
import java.util.jar.JarOutputStream
import org.gradle.api.provider.Provider

KOTLIN
  # Use the production service and factory; extraction drift must stop before Gradle runs.
  awk '
    /^abstract class KtlspCompilerRuntimes / { starts++; copying = 1 }
    copying && /^object KtlspCompilerConfiguration / { copying = 0; ends++ }
    copying { print }
    END {
      if (starts != 1 || ends != 1) {
        print "Compiler resource probe: expected one production runtime owner; extraction drifted" > "/dev/stderr"
        exit 1
      }
    }
  ' "$repo/scripts/classpath-dump.init.gradle.kts" >> "$project/build.gradle.kts" || return $?
  cat >> "$project/build.gradle.kts" <<'KOTLIN'

class CompilerResourceProbe(
    private val jar: File,
    private val runtimes: Provider<KtlspCompilerRuntimes>,
) : Action<Task>, java.io.Serializable {
    override fun execute(task: Task) {
        val expected = ByteArray(17783) { (it % 251).toByte() }
        check(jar.parentFile.mkdirs() || jar.parentFile.isDirectory)
        JarOutputStream(jar.outputStream()).use { output ->
            listOf("inspector" to byteArrayOf(42), "victim" to expected).forEach { (name, bytes) ->
                output.putNextEntry(JarEntry(name)); output.write(bytes); output.closeEntry()
            }
        }
        val urls = arrayOf(jar.toURI().toURL())
        for (production in listOf(false, true)) {
            URLClassLoader(urls, null).use { victim ->
                checkNotNull(victim.getResourceAsStream("victim")).use { pending ->
                    val inspector = if (production) KtlspCompilerRuntimes.open(listOf(jar), null) else URLClassLoader(urls, null)
                    check(inspector.parent == null && inspector.getURLs().contentEquals(urls))
                    inspector.use { loader ->
                        checkNotNull(loader.getResourceAsStream("inspector")).use { check(it.read() == 42) }
                    }
                    check(inspector.findResource("inspector") == null) { "Inspector loader was not closed" }
                    val result = runCatching { pending.readBytes() }
                    if (production) {
                        check(result.getOrThrow().contentEquals(expected)) { "Production inspector changed victim bytes" }
                    } else {
                        val failure = result.exceptionOrNull()
                        check(failure is IOException && failure.message.orEmpty().contains("closed", ignoreCase = true)) {
                            "Ordinary inspector did not invalidate the pending victim stream"
                        }
                    }
                }
            }
        }
        task.logger.quiet("PASS  compiler inspector closes without invalidating another loader's pending resource")
        val owner = runtimes.get()
        val owned = owner.inspector(listOf(jar), null)
        check(owned.parent == null && owner.inspector(listOf(jar), null) === owned)
        val second = File(jar.parentFile, "compiler-resource-second.jar")
        jar.copyTo(second, overwrite = true)
        val ordered = owner.inspector(listOf(jar, second), null)
        check(ordered !== owned && owner.inspector(listOf(jar), second) === ordered)
        check(owner.inspector(listOf(second, jar), null) !== ordered)
        check(ordered.getURLs().contentEquals(arrayOf(jar.toURI().toURL(), second.toURI().toURL())))
        URLClassLoader(urls, null).use { borrowed ->
            checkNotNull(borrowed.getResourceAsStream("victim")).use { pending ->
                checkNotNull(owned.getResourceAsStream("inspector")).use { check(it.read() == 42) }
                owner.close()
                check(pending.readBytes().contentEquals(expected))
                check(borrowed.findResource("victim") != null)
            }
        }
        check(owned.findResource("inspector") == null && ordered.findResource("inspector") == null)
        check(runCatching { owner.inspector(listOf(jar), null) }.isFailure)
        owner.close()
        task.logger.quiet("PASS  compiler runtime owner reuses exact ordered URLs and closes only its own loaders")
    }
}

class CompilerRuntimeMutationProbe(
    private val jar: File,
    private val directory: String,
    private val runtimes: Provider<KtlspCompilerRuntimes>,
) : Action<Task>, java.io.Serializable {
    override fun execute(task: Task) {
        check(jar.parentFile.mkdirs() || jar.parentFile.isDirectory)
        JarOutputStream(jar.outputStream()).use { output ->
            output.putNextEntry(JarEntry("runtime")); output.write(42); output.closeEntry()
        }
        val loader = runtimes.get().inspector(listOf(jar), null)
        checkNotNull(loader.getResourceAsStream("runtime")).use { check(it.read() == 42) }
        // Complete, valid model bytes precede the late runtime mutation/close failure.
        task.logger.quiet("PROJECT\t:mutation\t$directory\nBUILD\t:\nSOURCESET\tmain\nEND\t:mutation")
        val previous = jar.lastModified()
        val bytes = jar.readBytes()
        // Preserve size and mtime: only a content digest, rather than a stat key, detects this.
        bytes[bytes.lastIndex] = (bytes.last().toInt() xor 1).toByte()
        jar.writeBytes(bytes)
        check(jar.setLastModified(previous))
        task.logger.quiet("KTLSP_RUNTIME_MUTATED_AFTER_COMPLETE_MODEL")
    }
}

// Execute the production identity helper on real Gradle action wrappers. User actions
// deliberately compare equal; delegate/class/equality filtering would lose them.
run {
    val probe = tasks.create("ktlspObserverActionIdentityProbe")
    fun action() = object : Action<Task> {
        override fun execute(task: Task) {}
        override fun equals(other: Any?) = true
        override fun hashCode() = 0
    }
    probe.doLast(action())
    val baseline = probe.actions.toList()
    val observer = KtlspObserverActions()
    val start = action()
    observer.install(probe, true, start)
    val installed = probe.actions.first()
    observer.install(probe, false, action())
    val visible = observer.visible(probe)
    check(visible.size == baseline.size && visible.indices.all { visible[it] === baseline[it] })
    probe.doFirst(action())
    probe.doLast(start) // Same delegate, distinct user-installed wrapper: must remain visible.
    val changed = observer.visible(probe)
    check(changed.size == baseline.size + 2 && changed[1] === baseline[0])
    check(changed.first() === probe.actions.first() && changed.last() === probe.actions.last())
    val other = tasks.create("ktlspOtherActionIdentityProbe")
    other.actions.add(installed)
    check(observer.visible(other).single() === installed) // Exclusions are task-scoped.
    probe.actions.removeAt(probe.actions.indices.single { probe.actions[it] === installed })
    check(runCatching { observer.visible(probe) }.isFailure) // Missing own hook fails closed.
    println("PASS  observer-owned action identities preserve configured and late user hooks")
}

val compilerMutationRuntimes = gradle.sharedServices.registerIfAbsent("ktlspCompilerMutationRuntimes", KtlspCompilerRuntimes::class.java) {}
tasks.register("ktlspCompilerRuntimeMutationProbe") {
    usesService(compilerMutationRuntimes)
    doLast(CompilerRuntimeMutationProbe(layout.buildDirectory.file("compiler-runtime-mutated.jar").get().asFile, projectDir.absolutePath, compilerMutationRuntimes))
}

val compilerProbeRuntimes = gradle.sharedServices.registerIfAbsent("ktlspCompilerProbeRuntimes", KtlspCompilerRuntimes::class.java) {}
tasks.register("ktlspCompilerResourceProbe") {
    usesService(compilerProbeRuntimes)
    doLast(CompilerResourceProbe(layout.buildDirectory.file("compiler-resource-probe.jar").get().asFile, compilerProbeRuntimes))
}
KOTLIN
  printf '%s\n' "$project"
}

run_classpath_config_cache_probe() {
  local project="$1"
  local cold_out="$artifacts/classpath-config-cache-cold.out"
  local warm_out="$artifacts/classpath-config-cache-warm.out"
  local status=0

  (
    cd "$project"
    ./gradlew -I "$repo/scripts/classpath-dump.init.gradle.kts" \
      ktlspCompilerResourceProbe :app:ktlspDumpClasspath -q --console=plain --configuration-cache \
      --configuration-cache-problems=fail --offline
  ) >"$cold_out" 2>&1 || status=$?
  cat "$cold_out"
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  grep -q $'^PROJECT\t:app\t' "$cold_out" || return 1
  grep -q $'^UNRESOLVED\tcom.example:ktlsp-harness-missing:0.0.0\t' "$cold_out" || return 1
  grep -q '^PASS  compiler inspector closes without invalidating' "$cold_out" || return 1
  grep -q '^PASS  compiler runtime owner reuses exact ordered URLs' "$cold_out" || return 1

  (
    cd "$project"
    ./gradlew -I "$repo/scripts/classpath-dump.init.gradle.kts" \
      ktlspCompilerResourceProbe :app:ktlspDumpClasspath --console=plain --configuration-cache \
      --configuration-cache-problems=fail --offline --info
  ) >"$warm_out" 2>&1 || status=$?
  cat "$warm_out"
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  grep -q '^Reusing configuration cache\.$' "$warm_out" || return 1
  grep -q '^Configuration cache entry reused\.$' "$warm_out" || return 1
  grep -q $'^UNRESOLVED\tcom.example:ktlsp-harness-missing:0.0.0\t' "$warm_out" || return 1
  grep -q '^PASS  compiler inspector closes without invalidating' "$warm_out" || return 1
  echo "PASS  classpath dump handles late configuration creation and reuses configuration cache"
}

write_java_project() {
  local project="$projects/java"
  mkdir -p "$project/app" "$project/support" "$project/renamecase" "$project/barecase"
  cat > "$project/barecase/Base.java" <<'JAVA'
package barecase;
public class Base { public static int assist() { return 1; } }
JAVA
  cat > "$project/barecase/Child.java" <<'JAVA'
package barecase;
public class Child extends Base {
    public static int assist() { return 2; }
    public static int run() { return assist(); }
}
JAVA
  cat > "$project/barecase/Decoy.java" <<'JAVA'
package barecase;
public class Decoy { public static int assist() { return 3; } }
JAVA
  cat > "$project/barecase/JavaUse.java" <<'JAVA'
package barecase;
public class JavaUse { public static int run() { return Child.assist(); } }
JAVA
  cat > "$project/barecase/KotlinUse.kt" <<'KOTLIN'
package barecase
fun runKt(): Int = Child.assist()
KOTLIN
  cat > "$project/renamecase/Contract.java" <<'JAVA'
package renamecase;
public interface Contract { void assist(); }
JAVA
  cat > "$project/renamecase/Provider.java" <<'JAVA'
package renamecase;
public class Provider implements Contract {
    public void assist() { }
    public void assist(String text) { }
}
JAVA
  cat > "$project/renamecase/Unrelated.java" <<'JAVA'
package renamecase;
public class Unrelated { public void assist() { } }
JAVA
  cat > "$project/renamecase/JavaUse.java" <<'JAVA'
package renamecase;
public class JavaUse {
    public void run(Provider provider, Contract contract, Unrelated unrelated) {
        provider.assist(); contract.assist(); provider.assist("keep"); unrelated.assist();
    }
}
JAVA
  cat > "$project/renamecase/KotlinUse.kt" <<'KOTLIN'
package renamecase
fun runKt(provider: Provider, contract: Contract, unrelated: Unrelated) {
    provider.assist(); contract.assist(); provider.assist("keep"); unrelated.assist()
}
KOTLIN
  cat > "$project/app/Helper.java" <<'JAVA'
package app;

interface Worker {
    void assist();
}

public class Helper extends Base implements Worker {
    public Helper() { }
    public void assist() { }
    public String combine(String name, int count) { return name + count; }
    public void waitFor(int seconds) { }
    public void waitFor(int seconds, int nanos) { }
    public void adopt(Cat cat) { }
}

class Cat { }

class Dog { }

class Base {
    public void inherited() { }
}

class Other {
    public void assistUnrelated() { }
}
JAVA
  cat > "$project/app/KotlinHelper.kt" <<'KOTLIN'
package app

class KotlinHelper {
    fun assist() { }
}
KOTLIN
  cat > "$project/app/Main.java" <<'JAVA'
package app;

import java.util.List;

public class Main {
    public void run(String seed) {
        Helper helper = new Helper();
        var inferred = new Helper();
        Worker worker = helper;
        worker.assist();
        helper.assist();
        helper.inherited();
        this.runAgain();
        KotlinHelper kotlinHelper = new KotlinHelper();
        kotlinHelper.assist();
        helper.combine("Ada", 2);
        helper.waitFor(1);
        helper.combine("Ada");
        helper.adopt(new Dog());
        missingCall();
    }

    public void runAgain() { }
}
JAVA
  cat > "$project/support/ImportedBase.java" <<'JAVA'
package support;

public class ImportedBase {
    public void baseOnly() { }
    public void accept(String value) { }
}
JAVA
  cat > "$project/app/ImportedBase.java" <<'JAVA'
package app;

public class ImportedBase {
    public void baseWrong() { }
    public void accept(int value) { }
}
JAVA
  cat > "$project/app/ImportedChild.java" <<'JAVA'
package app;

import support.ImportedBase;

public class ImportedChild extends ImportedBase { }
JAVA
  cat > "$project/app/KotlinImportedChild.kt" <<'KOTLIN'
package app

import support.ImportedBase

class KotlinImportedChild : ImportedBase()
KOTLIN
  cat > "$project/app/SuperUse.java" <<'JAVA'
package app;

public class SuperUse {
    public void run(ImportedChild child, KotlinImportedChild kotlin) {
        child.baseOnly();
        child.accept("ok");
        kotlin.baseOnly();
        kotlin.accept("ok");
    }
}
JAVA
  cat > "$project/app/SuperUse.kt" <<'KOTLIN'
package app

fun useSuperclasses(child: ImportedChild, kotlin: KotlinImportedChild) {
    child.baseOnly()
    child.accept("ok")
    kotlin.baseOnly()
    kotlin.accept("ok")
}
KOTLIN
  mkdir -p "$project/resourcecase"
  cat > "$project/resourcecase/Bodies.kt" <<'KOTLIN'
package resourcecase

class Body : AutoCloseable {
    fun string(): String = "body"
    override fun close() { }
}

class Other {
    fun enqueue() { }
}
KOTLIN
  cat > "$project/resourcecase/ResourceUse.java" <<'JAVA'
package resourcecase;

public class ResourceUse {
    private Other body = new Other();

    public void run(Body supply) {
        /* before */ body.enqueue();
        try (Body body = supply) {
            System.out.println(body.string());
        } catch (Exception failure) {
            /* caught */ body.enqueue();
        } finally {
            /* finished */ body.enqueue();
        }
        /* after */ body.enqueue();
    }

    public void local(Body supply) {
        Body localBody = supply;
        System.out.println(localBody.string());
    }

    public void inferred(Body supply) {
        try (var inferredBody = supply) {
            System.out.println(inferredBody.string());
        }
    }

    private Body make() { return new Body(); }
    private <T> T identity(T value) { return value; }

    public void factory(Body supply) {
        var factoryBody = make();
        System.out.println(factoryBody.string());
        identity(supply).string();
    }

    public void kotlinLoop(Body[] values) {
        for (Body kotlinItem : values) {
            System.out.println(kotlinItem.string());
        }
    }

    public void javaLoop(JavaLoopBody[] values) {
        for (JavaLoopBody javaItem : values) {
            System.out.println(javaItem.javaOnly());
        }
    }

    public void varKotlinArray(Body[] values) {
        for (var varKotlinArrayItem : values) {
            System.out.println(varKotlinArrayItem.string());
        }
    }

    public void varJavaArray(JavaLoopBody[] values) {
        for (var varJavaArrayItem : values) {
            System.out.println(varJavaArrayItem.javaOnly());
        }
    }

    public void varKotlinIterable(Iterable<Body> values) {
        for (var varKotlinIterableItem : values) {
            System.out.println(varKotlinIterableItem.string());
        }
    }

    public void varJavaIterable(Iterable<JavaLoopBody> values) {
        for (var varJavaIterableItem : values) {
            System.out.println(varJavaIterableItem.javaOnly());
        }
    }
    private Body[] makeKotlinArray() { return new Body[0]; }
    private JavaLoopBody[] makeJavaArray() { return new JavaLoopBody[0]; }
    private Body makeKotlinPostfixed()[] { return new Body[0]; }
    private JavaLoopBody makeJavaPostfixed()[] { return new JavaLoopBody[0]; }

    public void varArrayMethods() {
        for (var kotlinMethodArrayItem : makeKotlinArray()) {
            System.out.println(kotlinMethodArrayItem.string());
        }
        for (var javaMethodArrayItem : makeJavaArray()) {
            System.out.println(javaMethodArrayItem.javaOnly());
        }
        for (var kotlinPostfixedArrayItem : makeKotlinPostfixed()) {
            System.out.println(kotlinPostfixedArrayItem.string());
        }
        for (var javaPostfixedArrayItem : makeJavaPostfixed()) {
            System.out.println(javaPostfixedArrayItem.javaOnly());
        }
    }

    public void varArrayCreation() {
        for (var kotlinCreatedArrayItem : new Body[0]) {
            System.out.println(kotlinCreatedArrayItem.string());
        }
        for (var javaCreatedArrayItem : new JavaLoopBody[0]) {
            System.out.println(javaCreatedArrayItem.javaOnly());
        }
    }

    public void varArrayAliases(Body[] kotlinValues, JavaLoopBody[] javaValues) {
        var kotlinDeclaredAlias = kotlinValues;
        for (var kotlinDeclaredAliasItem : kotlinDeclaredAlias) {
            System.out.println(kotlinDeclaredAliasItem.string());
        }
        var javaDeclaredAlias = javaValues;
        for (var javaDeclaredAliasItem : javaDeclaredAlias) {
            System.out.println(javaDeclaredAliasItem.javaOnly());
        }
        var kotlinMethodAlias = makeKotlinArray();
        for (var kotlinMethodAliasItem : kotlinMethodAlias) {
            System.out.println(kotlinMethodAliasItem.string());
        }
        var javaMethodAlias = makeJavaArray();
        for (var javaMethodAliasItem : javaMethodAlias) {
            System.out.println(javaMethodAliasItem.javaOnly());
        }
        var kotlinCreatedAlias = new Body[0];
        for (var kotlinCreatedAliasItem : kotlinCreatedAlias) {
            System.out.println(kotlinCreatedAliasItem.string());
        }
        var javaCreatedAlias = new JavaLoopBody[0];
        for (var javaCreatedAliasItem : javaCreatedAlias) {
            System.out.println(javaCreatedAliasItem.javaOnly());
        }
    }

    public void varArrayAliasRows() {
        var kotlinAliasRows = new Body[0][0];
        for (var kotlinAliasRow : kotlinAliasRows) {
            System.out.println(kotlinAliasRow.length);
        }
        var javaAliasRows = new JavaLoopBody[0][0];
        for (var javaAliasRow : javaAliasRows) {
            System.out.println(javaAliasRow.length);
        }
    }

    public void arrayMembers(Body[][] kotlinValues, JavaLoopBody[][] javaValues) {
        for (var kotlinMemberRow : kotlinValues) {
            System.out.println(kotlinMemberRow.length);
            System.out.println(kotlinMemberRow.clone().length);
            System.out.println(kotlinMemberRow.clone()[0].string());
        }
        for (var javaMemberRow : javaValues) {
            System.out.println(javaMemberRow.length);
            System.out.println(javaMemberRow.clone().length);
            System.out.println(javaMemberRow.clone()[0].javaOnly());
        }
    }

}

class JavaLoopBody {
    public String javaOnly() { return "java"; }
}
JAVA

  cat > "$project/resourcecase/ArraySdkUse.java" <<'JAVA'
package resourcecase;

import java.io.File;
import java.lang.Object;

public class ArraySdkUse {
    public void run(Body[][] rows, File directory) {
        Object[] objects = rows[0];
        String rowText = rows[0].toString();
        int rowHash = rows[0].hashCode();
        boolean same = rows[0].equals(objects);
        String cellText = rows[0].clone()[0].string();
        int entryCount = directory.listFiles().length;
        int clonedEntryCount = directory.listFiles().clone().length;
        String name = directory.listFiles().clone()[0].getName();
    }
}
JAVA

  cat > "$project/resourcecase/KotlinArrayFactory.java" <<'JAVA'
package resourcecase;
public class KotlinArrayFactory {
    public Body[][] matrix() { return null; }
}
JAVA
  cat > "$project/resourcecase/KotlinArrayUse.kt" <<'KOTLIN'
package resourcecase
fun kotlinArrayReads(factory: KotlinArrayFactory) {
    val total: Int = factory.matrix().size
    val row = factory.matrix()[0]
    val rowSize: Int = row.size
    factory.matrix()[0][0].string()
    factory.matrix().clone().get(0).get(0).string()
}
KOTLIN

  mkdir -p "$project/hints"
  cp "$repo/dev/corpus/fixtures/java-inlay-hints/HintUse.java" \
    "$repo/dev/corpus/fixtures/java-inlay-hints/Hints.kt" "$project/hints/"

  mkdir -p "$project/selection"
  cat > "$project/selection/Reader.java" <<'JAVA'
package selection;

public class Reader {
    public int body;
    public Payload body() { return new Payload(); }
    public static class Factory { }
    public static <T> String pickTop(java.util.List<T> values, T value) { return "generic"; }
    public static String pickTop(java.util.Collection<String> values, Object value) { return "collection"; }
    public static <T> String pickNested(java.util.List<T> first, java.util.List<T> second) { return "generic"; }
    public static String pickNested(java.util.Collection<String> first, java.util.Collection<String> second) { return "collection"; }
}
JAVA
  cat > "$project/selection/Payload.java" <<'JAVA'
package selection;

public class Payload {
    public void useful() { }
}
JAVA
  cat > "$project/selection/Unrelated.java" <<'JAVA'
package selection;

public class Unrelated {
    public static class Factory { }
}
JAVA
  cat > "$project/selection/MemberUse.java" <<'JAVA'
package selection;

public class MemberUse {
    public void run(Reader reader) {
        reader.body().useful();
        int value = reader.body;
        Reader.Factory factory = new Reader.Factory();
    }
    public void inferred(Callables callables, InferredBox<Product> box, InferredArrays arrays) {
        box.getValue().propertyOnly();
        box.setValue(new Product());
        callables.inferredProduct().propertyOnly();
        callables.getCallback();
        arrays.getValues()[0].propertyOnly();
        int length = arrays.getInts().length;
    }
    public <T> void classLiterals() {
        String directName = Runnable.class.getName();
        var literalClass = Runnable.class;
        String literalName = literalClass.getName();
        var primitiveClass = int.class;
        String primitiveName = primitiveClass.getName();
        var arrayClass = Runnable[].class;
        String arrayName = arrayClass.getName();
    }
}
JAVA
  cat > "$project/selection/Callable.kt" <<'KOTLIN'
package selection

class Product {
    fun propertyOnly() { }
}

class OtherResult {
    fun methodOnly() { }
}

class SmartItem {
    fun ping() { }
}

class InferredBox<T>(val input: T) {
    var value = input
}

class InferredArrays(seed: Array<Product>, numbers: IntArray) {
    val values = seed
    val ints = numbers
}

class Callables {
    val action: (Int) -> Product = { Product() }
    val contextual: (SmartItem) -> Unit = { it.ping() }
    val namedContextual: (SmartItem) -> Unit = { item -> item.ping() }
    fun action(value: String): OtherResult = OtherResult()
    val maybe: (String?) -> Product? = { null }
    val optional: ((Int) -> Product)? = null
    private fun product(): Product = Product()
    fun inferredProduct() = product()
    val callback = { item: SmartItem -> item }
}

fun useMembers(reader: Reader, callables: Callables) {
    reader.body().useful()
    val value = reader.body
    val factory: Reader.Factory = Reader.Factory()
    callables.action(1).propertyOnly()
    callables.action("x").methodOnly()
    callables.maybe(null)?.propertyOnly()
    val optional = callables.optional
}

fun useInferred(callables: Callables, box: InferredBox<Product>, item: SmartItem) {
    box.value.propertyOnly()
    callables.inferredProduct().propertyOnly()
    callables.callback(item).ping()
}

fun useTopSelection(values: java.util.List<String>, text: String) {
    Reader.pickTop(values, text).isEmpty()
}

fun useNestedSelection(values: java.util.List<String>, other: java.util.List<String>) {
    Reader.pickNested(values, other).isEmpty()
}
KOTLIN
  printf '%s\n' "$project"
}

write_fake_jdk_src_zip() {
  local src_root="$run_dir/fake-jdk-src"
  local src_zip="$run_dir/fake-jdk-src.zip"
  mkdir -p "$src_root/java/lang"
  cat > "$src_root/java/lang/Object.java" <<'JAVA'
package java.lang;

public class Object {
}
JAVA
  (cd "$src_root" && zip -qr "$src_zip" .)
  printf '%s\n' "$src_zip"
}

run_and_capture() {
  local name="$1"
  shift
  local out="$artifacts/$name.out"
  set +e
  "$@" >"$out" 2>&1
  local status=$?
  set -e
  cat "$out"
  return "$status"
}

run_completion_ui() {
  local ui_project="$1" ui_artifacts="$artifacts"
  local ui_script="$repo/dev/nvim_completion_ui.lua" ui_bin="$bin_arg"
  if command -v cygpath >/dev/null 2>&1; then
    ui_project="$(cygpath -m "$ui_project")"
    ui_artifacts="$(cygpath -m "$ui_artifacts")"
    ui_script="$(cygpath -m "$ui_script")"
    ui_bin="$(cygpath -m "$ui_bin")"
  fi
  KTLSP_UI_PROJECT="$ui_project" KTLSP_UI_ARTIFACTS="$ui_artifacts" \
    KTLSP_UI_SCRIPT="$ui_script" KTLSP_BIN="$ui_bin" \
    run_and_capture completion-ui nvim --headless -u NONE -i NONE -n \
      -c 'lua dofile(vim.env.KTLSP_UI_SCRIPT)'
}

postprocess() {
  if [ -x "$bench" ] && [ -s "$KTLSP_TRACE" ]; then
    "$bench" trace --file "$KTLSP_TRACE" --out "$artifacts/trace.json" >>"$artifacts/harness.log" 2>&1 || true
  fi
  local nvim_log="$XDG_STATE_HOME/nvim/lsp.log"
  if [ -f "$XDG_STATE_HOME/nvim-data/lsp.log" ]; then
    nvim_log="$XDG_STATE_HOME/nvim-data/lsp.log"
  fi
  {
    echo "scenario: $scenario"
    echo "status: $run_status"
    echo "run_dir: $run_dir"
    echo "ktlsp_bin: $bin_arg"
    echo "cache_dir: $KTLSP_CACHE_DIR"
    echo "trace_jsonl: $KTLSP_TRACE"
    echo "nvim_log: $nvim_log"
    if [ -f "$artifacts/trace.json" ]; then
      echo "trace_json: $artifacts/trace.json"
    fi
  } >"$artifacts/summary.txt"
  echo
  echo "ktlsp harness artifacts: $run_dir"
}

trap postprocess EXIT

case "$scenario" in
  basic)
    project="${root_arg:-$(write_basic_project)}"
    run_and_capture basic nvim -l "$repo/dev/nvim_smoke.lua" "$project" "$bin_arg" || run_status=$?
    ;;
  features)
    project="${root_arg:-$repo/dev/sample}"
    if [ -z "${KTLSP_JDK_SRC:-}" ]; then
      export KTLSP_JDK_SRC="$(write_fake_jdk_src_zip)"
    fi
    run_and_capture features nvim -l "$repo/dev/nvim_features.lua" "$project" || run_status=$?
    ;;
  library)
    project="${root_arg:-$(write_library_project)}"
    if [ -z "$root_arg" ]; then
      run_and_capture library-compile run_library_compile "$project" || run_status=$?
    fi
    if [ "$run_status" -eq 0 ]; then
      run_and_capture library nvim -l "$repo/dev/nvim_library.lua" "$project" || run_status=$?
    fi
    ;;
  java)
    project="${root_arg:-$(write_java_project)}"
    run_and_capture java nvim -l "$repo/dev/nvim_java.lua" "$project" "$bin_arg" "$artifacts" || run_status=$?
    ;;
  mixed)
    project="${root_arg:-$(write_mixed_project)}"
    run_and_capture mixed nvim -l "$repo/dev/nvim_mixed.lua" "$project" "$bin_arg" "$artifacts" || run_status=$?
    if [ "$run_status" -eq 0 ]; then
      run_completion_ui "$project" || run_status=$?
    fi
    ;;
  corpus)
    if [ -z "$root_arg" ] || [ -z "$file_arg" ]; then
      echo "corpus scenario requires --root and --file JSON manifest" >&2
      run_status=2
    else
      run_and_capture corpus nvim -l "$repo/dev/nvim_corpus.lua" "$root_arg" "$file_arg" "$bin_arg" "$artifacts" || run_status=$?
      if [ -n "${KTLSP_UI_CASES:-}" ]; then
        run_completion_ui "$root_arg" || run_status=$?
      fi
    fi
    ;;
  semantic)
    if [ -n "$root_arg" ]; then
      project="$root_arg"
    else
      project="$projects/semantic"
      mkdir -p "$project"
      cp -R "$repo/dev/semantic-fixture/." "$project/"
    fi
    run_and_capture semantic nvim -l "$repo/dev/nvim_semantic_fixture.lua" "$project" "$bin_arg" "$artifacts" || run_status=$?
    ;;
  project)
    if [ -z "$root_arg" ] || [ -z "$file_arg" ]; then
      echo "project scenario requires --root and --file" >&2
      run_status=2
    else
      run_and_capture project nvim -l "$repo/dev/nvim_project.lua" "$root_arg" "$file_arg" "$bin_arg" "$token_arg" "$occurrence_arg" "$target_arg" "$definition_token_arg" "$definition_occurrence_arg" "$definition_target_arg" || run_status=$?
    fi
    ;;
  java-project)
    if [ -z "$root_arg" ] || [ -z "$file_arg" ] || [ -z "$token_arg" ]; then
      echo "java-project scenario requires --root, --file, and --token" >&2
      run_status=2
    else
      run_and_capture java-project nvim -l "$repo/dev/nvim_java_project.lua" "$root_arg" "$file_arg" "$token_arg" "$occurrence_arg" "$bin_arg" || run_status=$?
    fi
    ;;
  emacs-project)
    if [ -z "$root_arg" ] || [ -z "$file_arg" ]; then
      echo "emacs-project scenario requires --root and --file" >&2
      run_status=2
    else
      run_and_capture emacs-project \
        emacs --batch -l "$repo/dev/emacs_project.el" -- \
        "$root_arg" "$file_arg" "$bin_arg" "${needle_arg:-KotlinLogging}" "${burst_arg:-6}" || run_status=$?
    fi
    ;;
  gradle-live|gradle-compile)
    if [ "${KTLSP_PROCESSOR_RELOAD:-0}" = "1" ]; then
      if [ -n "$root_arg" ]; then
        echo "Processor reload probe requires a new disposable project; omit --root" >&2
        run_status=2
      else
        run_and_capture "$scenario" nvim -l "$repo/dev/nvim_processor_reload.lua" \
          "$projects/processor-reload" "$bin_arg" "$artifacts" || run_status=$?
      fi
    elif [ "${KTLSP_GRADLE_RELOAD:-0}" = "1" ]; then
      if [ -n "$root_arg" ]; then
        echo "Gradle reload probe requires its generated disposable project; omit --root" >&2
        run_status=2
      else
        run_and_capture "$scenario" nvim -l "$repo/dev/nvim_gradle_reload.lua" \
          "$projects/gradle-reload" "$bin_arg" "$artifacts" || run_status=$?
      fi
    else
      project="${root_arg:-$repo/dev/gradle-sample}"
      classpath_project="$(write_classpath_config_cache_project)"
      run_classpath_config_cache_probe "$classpath_project" || run_status=$?
      if [ "$run_status" -eq 0 ]; then
        run_and_capture "$scenario" nvim -l "$repo/dev/nvim_gradle_live.lua" "$project" "$bin_arg" || run_status=$?
      fi
    fi
    ;;
  comprehensive)
    project="${root_arg:-$repo/dev/gradle-sample}"
    run_and_capture comprehensive nvim -l "$repo/dev/nvim_comprehensive.lua" "$project" "$bin_arg" || run_status=$?
    ;;
  *)
    echo "unknown scenario: $scenario" >&2
    usage
    run_status=2
    ;;
esac

exit "$run_status"
