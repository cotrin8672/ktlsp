import java.util.ArrayDeque
import org.gradle.api.artifacts.component.ComponentIdentifier
import org.gradle.api.artifacts.result.ResolvedComponentResult
import org.gradle.api.artifacts.result.ResolvedDependencyResult
import org.gradle.api.artifacts.result.UnresolvedDependencyResult

// Dump each module's resolved `compileClasspath` as a machine-readable line protocol, for ktlsp's
// Kotlin compile-daemon backend to feed the compiler. Run once and cache (the classpath is stable
// across .kt edits); re-run only when build files change.
//
//   ./gradlew -I scripts/classpath-dump.init.gradle.kts ktlspDumpClasspath -q
//
// Output (stdout), one record per module:
//   PROJECT\t<gradle path>\t<absolute projectDir>
//   CP\t<absolute jar/dir>          (repeated)
//   UNRESOLVED\t<selector>\t<problem> (lenient-resolution drops, repeated)
//   END\t<gradle path>
//
// `isLenient = true` so an unresolved dependency in one module doesn't abort the whole dump.
// Each block is buffered and printed with a single `println`: Gradle runs these tasks in parallel
// and per-line printing interleaves the protocol blocks, scrambling ktlsp's per-module
// attribution (observed: ~3.6k interleave anomalies in one GoodNotes dump). One synchronized
// PrintStream call per module keeps the block atomic.
//
// UNRESOLVED lines report what the lenient view dropped, so silent under-indexing is observable.
//
// Configuration-cache compatibility: all `Project` state (path, projectDir, the
// `compileClasspath` FileCollection, and its ResolutionResult root Provider) is captured at
// configuration time as a CC-supported value. The `doLast` action never calls `Task.getProject()`
// or touches a `Configuration` object. Watching the configuration collection also handles plugins
// that create `compileClasspath` from a later `afterEvaluate` callback. Modules without a
// `compileClasspath` configuration skip task registration entirely.
allprojects {
    val projectPath = project.path
    val projectDirPath = project.projectDir.absolutePath

    configurations.matching { it.name == "compileClasspath" }.all {
        val classpathFiles = incoming.artifactView {
            isLenient = true
        }.files
        val resolutionRoot = incoming.resolutionResult.rootComponent

        tasks.register("ktlspDumpClasspath") {
            doLast {
                val out = StringBuilder()
                out.append("PROJECT\t${projectPath}\t${projectDirPath}\n")
                classpathFiles.files.forEach {
                    out.append("CP\t${it.absolutePath}\n")
                }

                val pending = ArrayDeque<ResolvedComponentResult>()
                val visited = mutableSetOf<ComponentIdentifier>()
                val unresolved = linkedSetOf<String>()
                pending.add(resolutionRoot.get())
                while (pending.isNotEmpty()) {
                    val component = pending.removeFirst()
                    if (!visited.add(component.id)) {
                        continue
                    }
                    component.dependencies.forEach { dependency ->
                        when (dependency) {
                            is ResolvedDependencyResult -> pending.add(dependency.selected)
                            is UnresolvedDependencyResult -> {
                                val problem = dependency.failure.message
                                    ?.replace('\n', ' ')
                                    ?.replace('\t', ' ')
                                unresolved.add(
                                    "UNRESOLVED\t${dependency.attempted}\t${problem}"
                                )
                            }
                        }
                    }
                }

                unresolved.forEach {
                    out.append(it).append('\n')
                }
                out.append("END\t${projectPath}")
                println(out.toString())
            }
        }
    }
}
