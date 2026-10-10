import java.util.ArrayDeque
import groovy.json.JsonOutput
import org.gradle.api.Task
import org.gradle.api.Action
import org.gradle.api.file.FileCollection
import org.gradle.api.artifacts.component.ComponentIdentifier
import org.gradle.api.artifacts.component.BuildIdentifier
import org.gradle.api.artifacts.component.ModuleComponentIdentifier
import org.gradle.api.artifacts.component.ProjectComponentIdentifier
import org.gradle.api.artifacts.result.ResolvedComponentResult
import org.gradle.api.artifacts.result.ResolvedDependencyResult
import org.gradle.api.artifacts.result.UnresolvedDependencyResult
import org.gradle.api.file.SourceDirectorySet
import org.gradle.api.provider.Provider
import org.gradle.api.tasks.SourceSetContainer
import org.gradle.api.tasks.compile.JavaCompile
import org.gradle.process.CommandLineArgumentProvider

class KtlspSourceSet(
    val name: String,
    val metadata: Provider<List<String>>,
    val artifacts: Provider<List<String>>,
    val resolutionRoot: Provider<ResolvedComponentResult>,
)

object KtlspCompilerConfiguration {
    private fun read(bean: Any, getter: String): Any? {
        val method = bean.javaClass.methods.firstOrNull { it.name == getter && it.parameterCount == 0 }
            ?: error("Compiler task does not expose $getter")
        val value = method.invoke(bean)
        return if (value is Provider<*>) value.orNull else value
    }

    fun dump(task: Task, language: String): String {
        val record = linkedMapOf<String, Any?>(
            "task" to task.path, "language" to language, "task_type" to task.javaClass.name,
            "arguments" to emptyList<String>(), "annotation_processor_path" to emptyList<String>(),
            "java_home" to null, "options" to emptyMap<String, Any?>(),
            "output_roots" to emptyList<String>(), "complete" to false, "problem" to null,
        )
        try {
            if (task is JavaCompile) {
                val options = task.options
                record["arguments"] = options.allCompilerArgs.toList()
                record["annotation_processor_path"] = options.annotationProcessorPath?.files
                    ?.map { it.absolutePath } ?: emptyList<String>()
                record["java_home"] = task.javaCompiler.orNull?.metadata?.installationPath?.asFile?.absolutePath
                record["options"] = linkedMapOf(
                    "sourceCompatibility" to task.sourceCompatibility,
                    "targetCompatibility" to task.targetCompatibility,
                    "release" to options.release.orNull,
                    "encoding" to options.encoding,
                    "fork" to options.isFork,
                    "forkJavaHome" to options.forkOptions.javaHome?.absolutePath,
                    "forkExecutable" to options.forkOptions.executable,
                    "forkJvmArgs" to options.forkOptions.jvmArgs?.toList(),
                )
                record["output_roots"] = listOfNotNull(task.destinationDirectory.orNull?.asFile?.absolutePath)
            } else if (task.javaClass.methods.any { it.name == "getSerializedCompilerArguments" && it.parameterCount == 0 }) {
                // The public KGP getter keeps plugin options intact; unsupported versions stay unknown.
                val getter = task.javaClass.methods.firstOrNull {
                    it.name == "getSerializedCompilerArguments" && it.parameterCount == 0
                } ?: error("Kotlin task does not expose serialized compiler arguments")
                val arguments = getter.invoke(task) as? List<*>
                    ?: error("Kotlin compiler arguments are unavailable")
                check(arguments.all { it is String }) { "Unsupported Kotlin compiler argument value" }
                record["arguments"] = arguments.toList()
                val strings = arguments.filterIsInstance<String>()
                record["output_roots"] = strings.windowed(2).filter { it[0] == "-d" }.map { it[1] }
                record["java_home"] = strings.windowed(2).firstOrNull { it[0] == "-jdk-home" }?.get(1)
            } else {
                val processors = read(task, "getKaptClasspath") as? FileCollection
                    ?: error("Annotation processor classpath is unavailable")
                record["annotation_processor_path"] = processors.files.map { it.absolutePath }
                val plugins = read(task, "getKaptPluginOptions") as? List<*>
                    ?: error("KAPT plugin configuration is unavailable")
                val pluginOptions = plugins.map { plugin ->
                    read(checkNotNull(plugin), "getAsTaskInputArgs") as? Map<*, *>
                        ?: error("KAPT plugin options are unavailable")
                }
                val optionProviders = read(task, "getAnnotationProcessorOptionProviders") as? List<*>
                    ?: error("KAPT option providers are unavailable")
                val providerArguments = optionProviders.flatMap { provider ->
                    (provider as? CommandLineArgumentProvider)?.asArguments()?.toList()
                        ?: error("Unsupported KAPT option provider")
                }
                record["options"] = linkedMapOf(
                    "pluginOptions" to pluginOptions,
                    "javacOptions" to read(task, "getJavacOptions"),
                    "processors" to read(task, "getAnnotationProcessorFqNames"),
                    "includeCompileClasspath" to read(task, "getIncludeCompileClasspath"),
                    "classpath" to (read(task, "getClasspath") as FileCollection).files.map { it.absolutePath },
                    "providerArguments" to providerArguments,
                )
                record["output_roots"] = listOf("getClassesDir", "getDestinationDir", "getKotlinSourcesDestinationDir")
                    .mapNotNull { (read(task, it) as? org.gradle.api.file.Directory)?.asFile?.absolutePath }
            }
            record["complete"] = true
        } catch (failure: Exception) {
            record["problem"] = (failure.cause ?: failure).toString()
        }
        return JsonOutput.toJson(record)
    }
}

object KtlspBuildIdentity {
    fun path(build: BuildIdentifier?, currentRoot: String, includedRoots: Map<String, String>): String {
        if (build == null) return "legacy:${currentRoot}"
        // getBuildPath was added after Gradle 7; use only methods of the running public API.
        val api = BuildIdentifier::class.java.methods
        val path = api.firstOrNull { it.name == "getBuildPath" && it.parameterCount == 0 }
        if (path != null) return path.invoke(build) as String
        val current = api.firstOrNull { it.name == "isCurrentBuild" && it.parameterCount == 0 }
        if (current?.invoke(build) == true) return "legacy:${currentRoot}"
        val name = api.firstOrNull { it.name == "getName" && it.parameterCount == 0 }?.invoke(build) as? String
        val includedRoot = includedRoots[name]
        // An unexported nested build stays unmapped, rather than aliasing this build's project.
        return if (includedRoot != null) "legacy:${includedRoot}" else "unmapped:${currentRoot}|${name}"
    }
}

class KtlspDumpClasspathAction(
    private val projectPath: String,
    private val projectDirPath: String,
    private val currentBuildRoot: String,
    private val includedBuildRoots: Map<String, String>,
    private val classpathFiles: FileCollection,
    private val resolutionRoot: Provider<ResolvedComponentResult>,
    private val sourceSets: List<KtlspSourceSet>,
) : Action<Task> {
    override fun execute(task: Task) {
        val out = StringBuilder()
        out.append("PROJECT\t${projectPath}\t${projectDirPath}\n")
        val projectId = resolutionRoot.get().id as? ProjectComponentIdentifier
        out.append("BUILD\t${KtlspBuildIdentity.path(projectId?.build, currentBuildRoot, includedBuildRoots)}\n")
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
        System.out.println(out.toString())
    }
}

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
//   COMPILER\t<source set>\t<JSON task configuration, escaped opaque arguments>
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
    val currentBuildRoot = rootProject.projectDir.absolutePath
    val includedBuildRoots = gradle.includedBuilds.associate { it.name to it.projectDir.absolutePath }
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
                    // shortcut: these are JVM source-set task names; other compilation models need explicit adapters.
                    val kotlinTasks = listOf(
                        sourceSet.getCompileTaskName("kotlin"),
                        sourceSet.getTaskName("kapt", "Kotlin"),
                        sourceSet.getTaskName("kaptGenerateStubs", "Kotlin"),
                        sourceSet.getTaskName("ksp", "Kotlin"),
                    ).mapNotNull { taskName -> tasks.findByName(taskName) }.distinctBy { it.path }
                    val compilers = listOfNotNull(
                        compileJava?.let { KtlspCompilerConfiguration.dump(it, "java") },
                    ) + kotlinTasks.map { KtlspCompilerConfiguration.dump(it, "kotlin") }
                    compileJava?.options?.generatedSourceOutputDirectory?.orNull?.asFile?.let(roots::add)
                    // KAPT can feed generated Java directly to JavaCompile, outside allJava.
                    compileJava?.source?.visit {
                        if (!isDirectory) {
                            var inputRoot = file
                            relativePath.segments.forEach { inputRoot = inputRoot.parentFile }
                            roots.add(inputRoot)
                        }
                    }
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
                        compilers.map { "COMPILER\t${sourceSetName}\t${it}" } +
                        entries.map { "SCP\t${sourceSetName}\t${it.absolutePath}" } +
                        if (actualClasspath.isFailure) listOf("INCOMPLETE\t${sourceSetName}") else emptyList()
                }
                sourceSets.add(KtlspSourceSet(
                    sourceSetName, metadata, providers.provider {
                        // Read declared artifact paths without requiring project JAR tasks to run.
                        view.artifacts.artifacts.map { artifact ->
                            val identity = when (val id = artifact.id.componentIdentifier) {
                                is ModuleComponentIdentifier -> "MODULE\t${id.group}\t${id.module}\t${id.version}"
                                is ProjectComponentIdentifier -> "PROJECT\t${KtlspBuildIdentity.path(id.build, currentBuildRoot, includedBuildRoots)}\t${id.projectPath}"
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
            // Gradle 7 serializes enclosing Kotlin DSL closures, including their Project receiver.
            doLast(KtlspDumpClasspathAction(
                projectPath, projectDirPath, currentBuildRoot, includedBuildRoots,
                classpathFiles, resolutionRoot, sourceSets,
            ))
        }
    }
}
