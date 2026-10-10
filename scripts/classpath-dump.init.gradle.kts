import java.util.ArrayDeque
import org.gradle.api.artifacts.component.ComponentIdentifier
import org.gradle.api.artifacts.component.ModuleComponentIdentifier
import org.gradle.api.artifacts.component.ProjectComponentIdentifier
import org.gradle.api.artifacts.result.ResolvedComponentResult
import org.gradle.api.artifacts.result.ResolvedDependencyResult
import org.gradle.api.artifacts.result.UnresolvedDependencyResult
import org.gradle.api.file.SourceDirectorySet
import org.gradle.api.provider.Provider
import org.gradle.api.tasks.SourceSetContainer
import org.gradle.api.tasks.compile.JavaCompile

class KtlspSourceSet(
    val name: String,
    val metadata: Provider<List<String>>,
    val artifacts: Provider<List<String>>,
    val resolutionRoot: Provider<ResolvedComponentResult>,
)

// Dump each module's resolved `compileClasspath` as a machine-readable line protocol, for ktlsp's
// Kotlin compile-daemon backend to feed the compiler. Run once and cache (the classpath is stable
// across .kt edits); re-run only when build files change.
//
//   ./gradlew -I scripts/classpath-dump.init.gradle.kts ktlspDumpClasspath -q
//
// Output (stdout), one record per module:
//   PROJECT\t<gradle path>\t<absolute projectDir>
//   BUILD\t<build path>
//   CP\t<absolute jar/dir>          (repeated)
//   SOURCESET\t<name>
//   SOURCE\t<source set>\t<absolute source root>
//   OUTPUT\t<source set>\t<absolute output root>
//   SCP\t<source set>\t<absolute classpath entry> (in compilation order)
//   ARTIFACT\t<source set>\t<absolute file>\tMODULE\t<group>\t<name>\t<version>
//   ARTIFACT\t<source set>\t<absolute file>\tPROJECT\t<build path>\t<project path>
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
    val sourceSets = mutableListOf<KtlspSourceSet>()

    plugins.withId("java-base") {
        extensions.getByType(SourceSetContainer::class.java).all {
            val sourceSet = this
            val sourceSetName = name
            // A configuration-time provider sees later srcDir/output changes without retaining
            // SourceSet or Project in the cached task action.
            configurations.matching { it.name == sourceSet.compileClasspathConfigurationName }.all {
                val view = incoming.artifactView { isLenient = true }
                val metadata = providers.provider {
                    val roots = linkedSetOf<java.io.File>()
                    // Resources can contain Java/Kotlin examples that the compilers never read.
                    // Kotlin's SourceSet extension is either its directory set or a KotlinSourceSet
                    // exposing getKotlin(); inspect that public return type without a KGP dependency.
                    roots.addAll(sourceSet.allJava.srcDirs)
                    val kotlinSources = sourceSet.extensions.findByName("kotlin")
                    val kotlinDirectories = when (kotlinSources) {
                        is SourceDirectorySet -> kotlinSources
                        null -> null
                        else -> kotlinSources.javaClass.methods.firstOrNull {
                            it.name == "getKotlin" && it.parameterCount == 0 &&
                                SourceDirectorySet::class.java.isAssignableFrom(it.returnType)
                        }?.invoke(kotlinSources) as? SourceDirectorySet
                    }
                    kotlinDirectories?.srcDirs?.let(roots::addAll)
                    roots.addAll(sourceSet.output.generatedSourcesDirs.files)
                    val compileJava = tasks.findByName(sourceSet.compileJavaTaskName) as? JavaCompile
                    compileJava?.options?.generatedSourceOutputDirectory?.orNull?.asFile?.let(roots::add)
                    val outputs = sourceSet.output.files.toMutableSet()
                    compileJava?.destinationDirectory?.orNull?.asFile?.let(outputs::add)
                    val provided = linkedSetOf<java.io.File>()
                    listOf(sourceSet.apiElementsConfigurationName, sourceSet.runtimeElementsConfigurationName)
                        .mapNotNull { configurations.findByName(it) }.forEach { configuration ->
                            provided.addAll(configuration.outgoing.artifacts.files.files)
                            configuration.outgoing.variants.forEach {
                                provided.addAll(it.artifacts.files.files)
                            }
                        }
                    val actualClasspath = runCatching { sourceSet.compileClasspath.files }
                    val entries = actualClasspath.getOrElse { view.files.files }
                    roots.map { "SOURCE\t${sourceSetName}\t${it.absolutePath}" } +
                        outputs.map { "OUTPUT\t${sourceSetName}\t${it.absolutePath}" } +
                        provided.map { "PROVIDES\t${sourceSetName}\t${it.absolutePath}" } +
                        entries.map { "SCP\t${sourceSetName}\t${it.absolutePath}" } +
                        if (actualClasspath.isFailure) listOf("INCOMPLETE\t${sourceSetName}") else emptyList()
                }
                sourceSets.add(KtlspSourceSet(
                    sourceSetName, metadata, view.artifacts.resolvedArtifacts.map { artifacts ->
                        artifacts.map { artifact ->
                            val identity = when (val id = artifact.id.componentIdentifier) {
                                is ModuleComponentIdentifier -> "MODULE\t${id.group}\t${id.module}\t${id.version}"
                                is ProjectComponentIdentifier -> "PROJECT\t${id.build.buildPath}\t${id.projectPath}"
                                else -> "FILE"
                            }
                            "ARTIFACT\t${sourceSetName}\t${artifact.file.absolutePath}\t${identity}"
                        }
                    },
                    incoming.resolutionResult.rootComponent,
                ))
            }
        }
    }

    configurations.matching { it.name == "compileClasspath" }.all {
        val classpathFiles = incoming.artifactView {
            isLenient = true
        }.files
        val resolutionRoot = incoming.resolutionResult.rootComponent

        tasks.register("ktlspDumpClasspath") {
            doLast {
                val out = StringBuilder()
                out.append("PROJECT\t${projectPath}\t${projectDirPath}\n")
                val projectId = resolutionRoot.get().id as? ProjectComponentIdentifier
                out.append("BUILD\t${projectId?.build?.buildPath ?: ":"}\n")
                classpathFiles.files.forEach {
                    out.append("CP\t${it.absolutePath}\n")
                }
                sourceSets.forEach { sourceSet ->
                    out.append("SOURCESET\t${sourceSet.name}\n")
                    sourceSet.metadata.get().forEach { out.append(it).append('\n') }
                    sourceSet.artifacts.get().forEach { out.append(it).append('\n') }
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
                sourceSets.forEach { sourceSet ->
                    val queue = ArrayDeque<ResolvedComponentResult>()
                    val seen = mutableSetOf<ComponentIdentifier>()
                    val failures = linkedSetOf<String>()
                    queue.add(sourceSet.resolutionRoot.get())
                    while (queue.isNotEmpty()) {
                        val component = queue.removeFirst()
                        if (!seen.add(component.id)) continue
                        component.dependencies.forEach { dependency ->
                            when (dependency) {
                                is ResolvedDependencyResult -> queue.add(dependency.selected)
                                is UnresolvedDependencyResult -> {
                                    val problem = dependency.failure.message?.replace('\n', ' ')?.replace('\t', ' ')
                                    failures.add("SUNRESOLVED\t${sourceSet.name}\t${dependency.attempted}\t${problem}")
                                }
                            }
                        }
                    }
                    failures.forEach { out.append(it).append('\n') }
                }
                out.append("END\t${projectPath}")
                println(out.toString())
            }
        }
    }
}
