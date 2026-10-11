import java.util.ArrayDeque
import java.net.URLClassLoader
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

    private fun kotlinCompilerRecipe(task: Task, toolchain: Any, toolchainJavaHome: java.io.File): Map<String, Any?> {
        // This is KGP's actual ordered compiler runtime input, separate from target libraries.
        val compilerClasspath = (read(task, "getDefaultCompilerClasspath\$kotlin_gradle_plugin_common") as? FileCollection)
            ?.toList() ?: error("Kotlin compiler runtime classpath is unavailable")
        check(compilerClasspath.isNotEmpty()) { "Kotlin compiler runtime classpath is empty" }
        check(compilerClasspath.all { it.isAbsolute }) { "Kotlin compiler runtime paths must be absolute" }
        val tools = read(toolchain, "getCurrentJvmJdkToolsJar\$kotlin_gradle_plugin_common")
        check(tools == null || tools is java.io.File) { "Unsupported Kotlin compiler tools JAR value" }
        val toolsJar = tools as? java.io.File
        check(toolsJar == null || toolsJar.isAbsolute) { "Kotlin compiler tools JAR path must be absolute" }
        val gradleJvm = read(toolchain, "getGradleJvm\$kotlin_gradle_plugin_common")
            ?: error("Kotlin compiler Gradle JVM is unavailable")
        val gradleJavaHome = read(gradleJvm, "getJavaHome") as? java.io.File
            ?: error("Kotlin compiler Gradle JDK home is unavailable")
        check(gradleJavaHome.isAbsolute) { "Kotlin compiler Gradle JDK home must be absolute" }
        val runViaBuildToolsApi = read(task, "getRunViaBuildToolsApi\$kotlin_gradle_plugin_common") as? Boolean
            ?: error("Kotlin compiler Build Tools API policy is unavailable")
        val executionStrategy = (read(task, "getCompilerExecutionStrategy") as? Enum<*>)?.name
            ?: error("Kotlin compiler execution strategy is unavailable")
        check(executionStrategy in setOf("DAEMON", "IN_PROCESS", "OUT_OF_PROCESS")) {
            "Unsupported Kotlin compiler execution strategy: $executionStrategy"
        }
        val daemonArguments = read(task, "getNormalizedKotlinDaemonJvmArguments") as? List<*>
            ?: error("Kotlin compiler daemon JVM arguments are unavailable")
        check(daemonArguments.all { it is String }) { "Unsupported Kotlin compiler daemon JVM argument value" }
        val compilerVersion = try {
            // No parent loader: the KGP plugin version must not stand in for the selected compiler.
            URLClassLoader((compilerClasspath + listOfNotNull(toolsJar)).map { it.toURI().toURL() }.toTypedArray(), null).use { loader ->
                val versionClass = Class.forName("org.jetbrains.kotlin.config.KotlinCompilerVersion", true, loader)
                val version = versionClass.getMethod("getVersion").invoke(null) as? String
                    ?: error("Selected Kotlin compiler version is unavailable")
                check(version.isNotBlank() && version != "@snapshot@" && version != "<unknown>") {
                    "Selected Kotlin compiler version is unavailable: $version"
                }
                Class.forName("org.jetbrains.kotlin.cli.jvm.K2JVMCompiler", false, loader)
                version
            }
        } catch (failure: LinkageError) {
            throw IllegalStateException("Selected Kotlin compiler runtime cannot be loaded", failure)
        }
        return linkedMapOf(
            "default_classpath" to compilerClasspath.map { it.absolutePath },
            "current_jvm_tools_jar" to toolsJar?.absolutePath,
            "compiler_version" to compilerVersion,
            "gradle_java_home" to gradleJavaHome.absolutePath,
            "toolchain_java_home" to toolchainJavaHome.absolutePath,
            "run_via_build_tools_api" to runViaBuildToolsApi,
            "execution_strategy" to executionStrategy,
            "normalized_daemon_jvm_arguments" to daemonArguments.toList(),
        )
    }

    fun dump(task: Task, language: String): String {
        val record = linkedMapOf<String, Any?>(
            "task" to task.path, "language" to language, "task_type" to task.javaClass.name,
            "arguments" to emptyList<String>(), "annotation_processor_path" to emptyList<String>(),
            "source_files" to emptyList<String>(), "classpath" to emptyList<String>(),
            "declared_input_files" to emptyList<String>(), "declared_output_roots" to emptyList<String>(),
            "java_home" to null, "options" to emptyMap<String, Any?>(),
            "output_roots" to emptyList<String>(), "complete" to false, "problem" to null,
        )
        try {
            // Keep read and write roles separate; a configured path is not an execution receipt.
            record["declared_input_files"] = task.inputs.files.files.map { it.absolutePath }.sorted()
            record["declared_output_roots"] = task.outputs.files.files.map { it.absolutePath }.sorted()
            if (task is JavaCompile) {
                record["source_files"] = task.source.files.map { it.absolutePath }.sorted()
                record["classpath"] = task.classpath.files.map { it.absolutePath }

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
                // Serialized arguments omit these configured task inputs.
                record["source_files"] = listOf(
                    "getSources", "getJavaSources", "getScriptSources\$kotlin_gradle_plugin_common",
                ).flatMap { sourceGetter ->
                    (read(task, sourceGetter) as? FileCollection)?.files
                        ?: error("Kotlin compiler sources are unavailable: $sourceGetter")
                }.map { it.absolutePath }.distinct().sorted()
                record["classpath"] = (read(task, "getLibraries") as? FileCollection)?.files
                    ?.map { it.absolutePath } ?: error("Kotlin compiler classpath is unavailable")
                val destination = read(task, "getDestinationDirectory") as? org.gradle.api.file.Directory
                    ?: error("Kotlin compiler destination is unavailable")
                record["output_roots"] = listOf(destination.asFile.absolutePath)
                // shortcut: KGP's selected JDK uses an internal ABI; verify new layouts before supporting them.
                val toolchain = read(task, "getKotlinJavaToolchainProvider")
                    ?: error("Kotlin compiler toolchain is unavailable")
                val jvm = read(toolchain, "getBuildJvm\$kotlin_gradle_plugin_common")
                    ?: error("Kotlin compiler JVM is unavailable")
                val javaHome = read(jvm, "getJavaHome") as? java.io.File
                    ?: error("Kotlin compiler JDK home is unavailable")
                check(javaHome.isAbsolute) { "Kotlin compiler JDK home must be absolute" }
                val strings = arguments.filterIsInstance<String>()
                val explicitHomes = strings.mapIndexedNotNull { index, argument ->
                    when {
                        argument == "-jdk-home" -> strings.getOrNull(index + 1)
                            ?: error("Kotlin compiler JDK argument is missing its value")
                        argument.startsWith("-jdk-home=") -> argument.substringAfter('=')
                        else -> null
                    }
                }
                check(explicitHomes.size <= 1) { "Ambiguous Kotlin compiler JDK arguments" }
                val explicitHome = explicitHomes.singleOrNull()?.let { java.io.File(it) }
                check(explicitHome == null || explicitHome.isAbsolute) { "Kotlin compiler JDK argument must be absolute" }
                record["java_home"] = if ("-no-jdk" in strings) null else (explicitHome ?: javaHome).absolutePath
                // Assign only a complete recipe; capture failures retain source ownership and an explicit problem.
                record["kotlin_compiler"] = kotlinCompilerRecipe(task, toolchain, javaHome)
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
// Metadata is resolved lazily inside each owner project's dump task action; resolving another
// project's compiler configurations eagerly can violate Gradle's owner-lock requirement. The importer
// disables configuration caching until cached provider values are proved to refresh compiler recipes
// and source membership. Watching the configuration collection also handles plugins that create
// `compileClasspath` from a later `afterEvaluate` callback. Modules without that configuration skip
// task registration entirely.
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

// Selected JavaCompile observation is opt-in and requires --no-configuration-cache.
// Configured compiler tasks alone do not certify successful or fresh outputs.
object KtlspSelectedTaskObserver {
    private var sequence = 0
    lateinit var directory: java.io.File
    lateinit var nonce: String

    fun path(file: java.io.File): String = file.toPath().toAbsolutePath().normalize().toString().replace('\\', '/')

    fun sha(file: java.io.File): String {
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        file.inputStream().buffered().use { input ->
            val buffer = ByteArray(65536)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                digest.update(buffer, 0, count)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it.toInt() and 255) }
    }

    fun file(file: java.io.File): Map<String, Any?> {
        val location = file.toPath()
        check(!java.nio.file.Files.isSymbolicLink(location)) { "Unsupported symbolic link: $location" }
        if (!java.nio.file.Files.exists(location, java.nio.file.LinkOption.NOFOLLOW_LINKS)) {
            return linkedMapOf("path" to path(file), "kind" to "absent")
        }
        check(java.nio.file.Files.isRegularFile(location, java.nio.file.LinkOption.NOFOLLOW_LINKS)) {
            "Unsupported non-regular file: $location"
        }
        return linkedMapOf("path" to path(file), "kind" to "file", "bytes" to file.length(), "sha256" to sha(file))
    }

    fun inventory(root: java.io.File): Map<String, Any?> {
        if (!root.exists() || root.isFile) return file(root)
        check(!java.nio.file.Files.isSymbolicLink(root.toPath())) { "Unsupported directory link: $root" }
        val entries = mutableListOf<Map<String, Any?>>()
        java.nio.file.Files.walk(root.toPath()).use { stream ->
            val iterator = stream.iterator()
            while (iterator.hasNext()) {
                val current = iterator.next()
                check(!java.nio.file.Files.isSymbolicLink(current)) { "Unsupported link: $current" }
                if (java.nio.file.Files.isDirectory(current, java.nio.file.LinkOption.NOFOLLOW_LINKS)) continue
                check(entries.size < 20000) { "Inventory cap exceeded: $root" }
                entries.add(file(current.toFile()))
            }
        }
        entries.sortBy { it["path"].toString() }
        return linkedMapOf("path" to path(root), "kind" to "directory", "entries" to entries)
    }

    fun inventory(roots: Collection<java.io.File>): List<Map<String, Any?>> = roots.map { inventory(it) }

    fun ancestorConfigs(sources: Collection<java.io.File>): List<Map<String, Any?>> {
        val locations = linkedSetOf<java.io.File>()
        sources.filter { it.extension == "java" }.forEach { source ->
            var parent: java.io.File? = source.absoluteFile.parentFile
            while (parent != null) {
                locations.add(java.io.File(parent, "lombok.config"))
                parent = parent.parentFile
            }
        }
        return locations.sortedBy { path(it) }.map { file(it) }
    }

    fun compiler(task: Task): Boolean = task is JavaCompile || task.javaClass.methods.any {
        it.name == "getSerializedCompilerArguments" && it.parameterCount == 0
    }

    fun configuration(task: Task): Map<*, *> = groovy.json.JsonSlurper().parseText(
        KtlspCompilerConfiguration.dump(task, if (task is JavaCompile) "java" else "kotlin")
    ) as Map<*, *>

    fun pluginFiles(configurations: List<Map<*, *>>): List<Map<String, Any?>> {
        val paths = configurations.flatMap { configuration ->
            (configuration["arguments"] as? List<*>)?.filterIsInstance<String>()?.flatMap { argument ->
                if (argument.startsWith("-Xplugin=")) argument.removePrefix("-Xplugin=").split(',') else emptyList()
            } ?: emptyList()
        }.distinct()
        return paths.map { file(java.io.File(it)) }
    }

    fun javaInputs(task: JavaCompile, sourceSet: org.gradle.api.tasks.SourceSet,
                   compilerTasks: List<Task>, buildFiles: List<java.io.File>): Map<String, Any?> {
        val kotlinExtension = sourceSet.extensions.findByName("kotlin")
        val kotlinDirectories = when (kotlinExtension) {
            is SourceDirectorySet -> kotlinExtension
            null -> null
            else -> kotlinExtension.javaClass.methods.firstOrNull {
                it.name == "getKotlin" && it.parameterCount == 0 &&
                    SourceDirectorySet::class.java.isAssignableFrom(it.returnType)
            }?.invoke(kotlinExtension) as? SourceDirectorySet
        }
        val sources = (sourceSet.allJava.files + (kotlinDirectories?.files ?: emptySet()) + task.source.files)
            .sortedBy { path(it) }
        val configurations = compilerTasks.map { configuration(it) }
        val homes = configurations.mapNotNull { it["java_home"] as? String }.toMutableSet()
        homes.add(System.getProperty("java.home"))
        val runtimeFiles = homes.sorted().flatMap { home ->
            listOf("release", "bin/java", "bin/java.exe", "bin/javac", "bin/javac.exe", "lib/modules")
                .map { file(java.io.File(home, it)) }
        }
        return linkedMapOf(
            "configuration" to configuration(task),
            "compiler_configurations" to configurations,
            "compiler_runtime_files" to runtimeFiles,
            "module_sources" to sources.map { file(it) },
            "java_task_sources" to task.source.files.sortedBy { path(it) }.map { file(it) },
            "ordered_classpath" to inventory(task.classpath.files.toList()),
            "annotation_processor_path" to inventory(task.options.annotationProcessorPath?.files?.toList() ?: emptyList()),
            "plugin_artifacts" to pluginFiles(configurations),
            "declared_task_input_files" to inventory(task.inputs.files.files.sortedBy { path(it) }),
            "build_configuration_files" to buildFiles.map { file(it) },
            "ancestor_lombok_configs" to ancestorConfigs(sources),
            "gradle_java_home" to System.getProperty("java.home"),
            "gradle_java_version" to System.getProperty("java.version"),
        )
    }

    fun state(state: org.gradle.api.tasks.TaskState): Map<String, Any?> = linkedMapOf(
        "executed" to state.executed, "failure" to state.failure?.toString(),
        "skipped" to state.skipped, "skip_message" to state.skipMessage,
        "up_to_date" to state.upToDate, "no_source" to state.noSource, "did_work" to state.didWork,
    )

    @Synchronized fun event(name: String, values: Map<String, Any?>) {
        val record = linkedMapOf<String, Any?>("nonce" to nonce, "sequence" to sequence,
            "event" to name, "nano_time" to System.nanoTime(), "utc" to java.time.Instant.now().toString())
        record.putAll(values)
        val target = java.io.File(directory, "%04d-%s.json".format(sequence++, name))
        java.nio.file.Files.write(target.toPath(), JsonOutput.toJson(record).toByteArray(Charsets.UTF_8),
            java.nio.file.StandardOpenOption.CREATE_NEW, java.nio.file.StandardOpenOption.WRITE)
    }
}

val ktlspObserverDirectory = System.getProperty("ktlsp.observer.dir")
val ktlspObserverRoot = System.getProperty("ktlsp.observer.root")
val ktlspObserverTask = System.getProperty("ktlsp.observer.task")
val ktlspObserverNonce = System.getProperty("ktlsp.observer.nonce")
if (ktlspObserverDirectory != null && ktlspObserverRoot != null && ktlspObserverTask != null && ktlspObserverNonce != null) {
    gradle.taskGraph.whenReady(object : Action<org.gradle.api.execution.TaskExecutionGraph> {
        override fun execute(graph: org.gradle.api.execution.TaskExecutionGraph) {
            if (gradle.rootProject.rootDir.canonicalFile != java.io.File(ktlspObserverRoot).canonicalFile) return
            val observer = KtlspSelectedTaskObserver
            observer.directory = java.io.File(ktlspObserverDirectory)
            observer.nonce = ktlspObserverNonce
            check(observer.directory.isDirectory) { "Observer directory must be created by the controller" }
            val selected = graph.allTasks.single { it.path == ktlspObserverTask }
            check(selected is JavaCompile) { "Selected task is not a JavaCompile" }
            val sourceSet = selected.project.extensions.getByType(SourceSetContainer::class.java).single {
                it.compileJavaTaskName == selected.name
            }
            val pending = java.util.ArrayDeque<Task>()
            val dependencies = linkedSetOf<Task>()
            pending.add(selected)
            while (!pending.isEmpty()) {
                val current = pending.removeFirst()
                current.taskDependencies.getDependencies(current).forEach { dependency ->
                    if (dependencies.add(dependency)) pending.add(dependency)
                }
            }
            val compilerTasks = graph.allTasks.filter { (it == selected || it in dependencies) && observer.compiler(it) }
            val buildFiles = listOf("build.gradle.kts", "settings.gradle.kts", "gradle.properties", "gradlew",
                "gradlew.bat", "gradle/wrapper/gradle-wrapper.properties", "gradle/wrapper/gradle-wrapper.jar")
                .map { java.io.File(selected.project.rootDir, it) }
            observer.event("graph", linkedMapOf(
                "root" to observer.path(gradle.rootProject.rootDir), "project" to selected.project.path,
                "project_dir" to observer.path(selected.project.projectDir), "source_set" to sourceSet.name,
                "selected_task" to selected.path, "selected_task_class" to selected.javaClass.name,
                "gradle_version" to gradle.gradleVersion,
                "gradle_home" to gradle.gradleHomeDir?.let { observer.path(it) },
                "gradle_user_home" to observer.path(gradle.gradleUserHomeDir),
                "init_scripts" to gradle.startParameter.initScripts.map { observer.file(it) },
                "compiler_configurations" to compilerTasks.map { observer.configuration(it) },
            ))
            var selectedStarted = false
            selected.doFirst(object : Action<Task> {
                override fun execute(task: Task) {
                    selectedStarted = true
                    observer.event("selected-action-start", linkedMapOf("task" to selected.path,
                        "inputs" to observer.javaInputs(selected, sourceSet, compilerTasks, buildFiles),
                        "outputs_before" to observer.inventory(selected.outputs.files.files.toList())))
                }
            })
            selected.doLast(object : Action<Task> {
                override fun execute(task: Task) {
                    observer.event("selected-action-end", linkedMapOf("task" to selected.path,
                        "outputs" to observer.inventory(selected.outputs.files.files.toList())))
                }
            })
            // Retain the execution boundary callback on supported older Gradle versions.
            @Suppress("DEPRECATION")
            graph.addTaskExecutionListener(object : org.gradle.api.execution.TaskExecutionListener {
                override fun beforeExecute(task: Task) {
                    val record = linkedMapOf<String, Any?>("task" to task.path, "type" to task.javaClass.name)
                    if (task in compilerTasks) {
                        record["configuration"] = observer.configuration(task)
                        record["declared_inputs"] = observer.inventory(task.inputs.files.files.sortedBy { observer.path(it) })
                    }
                    observer.event("task-start", record)
                }
                override fun afterExecute(task: Task, state: org.gradle.api.tasks.TaskState) {
                    val record = linkedMapOf<String, Any?>("task" to task.path, "state" to observer.state(state))
                    if (task in compilerTasks) record["outputs"] = observer.inventory(task.outputs.files.files.toList())
                    if (task == selected && selectedStarted) record["inputs_after"] = observer.javaInputs(selected, sourceSet, compilerTasks, buildFiles)
                    observer.event("task-end", record)
                }
            })
            @Suppress("DEPRECATION")
            gradle.buildFinished(object : Action<org.gradle.BuildResult> {
                override fun execute(result: org.gradle.BuildResult) {
                    observer.event("build-finished", linkedMapOf("failure" to result.failure?.toString(),
                        "selected_started" to selectedStarted, "selected_state" to observer.state(selected.state),
                        "selected_outputs" to observer.inventory(selected.outputs.files.files.toList())))
                }
            })
        }
    })
}
