import java.util.ArrayDeque
import java.net.URLClassLoader
import groovy.json.JsonOutput
import org.gradle.api.Task
import org.gradle.api.Project
import org.gradle.api.Action
import org.gradle.api.file.FileCollection
import org.gradle.api.artifacts.component.ComponentIdentifier
import org.gradle.api.artifacts.component.BuildIdentifier
import org.gradle.api.artifacts.component.ModuleComponentIdentifier
import org.gradle.api.artifacts.component.ProjectComponentIdentifier
import org.gradle.api.artifacts.result.ResolvedComponentResult
import org.gradle.api.artifacts.result.ResolvedArtifactResult
import org.gradle.api.component.Component
import org.gradle.api.component.Artifact
import org.gradle.jvm.JvmLibrary
import org.gradle.language.base.artifact.SourcesArtifact
import org.gradle.maven.MavenModule
import org.gradle.maven.MavenPomArtifact
import org.gradle.api.artifacts.result.ResolvedDependencyResult
import org.gradle.api.artifacts.result.UnresolvedDependencyResult
import org.gradle.api.file.SourceDirectorySet
import org.gradle.api.provider.Provider
import org.gradle.api.tasks.SourceSet
import org.gradle.api.tasks.SourceSetContainer
import org.gradle.api.tasks.compile.JavaCompile

class KtlspSourceSet(
    val name: String,
    val metadata: Provider<List<String>>,
    val artifacts: Provider<List<String>>,
    val resolutionRoot: Provider<ResolvedComponentResult>,
)

object KtlspCompilerConfiguration {
    private data class JavaGradleProfile(
        val version: String,
        val configurationCacheGetter: String,
        val incrementalAction: String,
    )

    private fun javaGradleProfile(task: Task): JavaGradleProfile = when (task.project.gradle.gradleVersion) {
        "7.4.2" -> JavaGradleProfile("7.4.2", "isConfigurationCache", "IncrementalInputsTaskAction")
        "7.5" -> JavaGradleProfile("7.5", "isConfigurationCache", "IncrementalInputsTaskAction")
        "9.6.1" -> JavaGradleProfile("9.6.1", "isConfigurationCacheRequested", "IncrementalTaskAction")
        else -> error("Unsupported Java recipe Gradle adapter")
    }

    private fun defaultJavaHome(task: JavaCompile): java.io.File? {
        // shortcut: other Gradle defaults stay unknown; extend after verifying their compiler selector.
        if (task.project.gradle.gradleVersion !in listOf("7.4.2", "7.5") || task.javaClass.name !in listOf(
                "org.gradle.api.tasks.compile.JavaCompile",
                "org.gradle.api.tasks.compile.JavaCompile_Decorated",
            )) return null
        val options = task.options
        if (task.javaCompiler.isPresent || options.isFork || options.forkOptions.javaHome != null ||
            options.forkOptions.executable != null || !options.forkOptions.jvmArgs.isNullOrEmpty()) return null
        return runCatching {
            // Use the same Gradle classloader and singleton as the actual default compiler selection.
            val type = Class.forName("org.gradle.internal.jvm.Jvm", false, JavaCompile::class.java.classLoader)
            val current = type.getMethod("current").invoke(null)
            if (type.getMethod("isJdk").invoke(current) != true) return@runCatching null
            val home = type.getMethod("getJavaHome").invoke(current) as? java.io.File
            home?.takeIf { it.isAbsolute && it.isDirectory }
        }.getOrNull()
    }

    private fun javaConfiguredState(task: JavaCompile): List<Any?> {
        val o = task.options
        return listOf(
            task.source.files.toList(), task.classpath.files.toList(), task.sourceCompatibility, task.targetCompatibility,
            task.javaCompiler.isPresent, task.destinationDirectory.orNull?.asFile, task.modularity.inferModulePath.orNull,
            o.release.orNull, o.encoding, o.sourcepath?.toList(), o.bootstrapClasspath?.toList(), o.extensionDirs,
            o.isDebug, o.debugOptions.debugLevel, o.isWarnings, o.isDeprecation, o.isVerbose, o.isListFiles,
            o.isFailOnError, o.isIncremental, o.compilerArgs.toList(), o.allCompilerArgs.toList(),
            o.compilerArgumentProviders.toList(), o.annotationProcessorPath?.toList(),
            o.generatedSourceOutputDirectory.orNull?.asFile, o.headerOutputDirectory.orNull?.asFile,
            o.javaModuleVersion.orNull, o.javaModuleMainClass.orNull, o.isFork,
            o.forkOptions.javaHome, o.forkOptions.executable, o.forkOptions.jvmArgs?.toList(),
            o.forkOptions.allJvmArgs?.toList(), o.forkOptions.jvmArgumentProviders.toList(),
            o.forkOptions.memoryInitialSize, o.forkOptions.memoryMaximumSize,
            o.forkOptions.tempDir,
        )
    }

    private fun javaCompilerRecipe(task: JavaCompile, selectedHome: java.io.File?): Map<String, Any?> {
        // shortcut: pinned Gradle/current-SDK profiles; other policies remain unavailable.
        val profile = javaGradleProfile(task)
        check(read(task.project.gradle.startParameter, profile.configurationCacheGetter) == false) {
            "Java recipe capture requires configuration cache disabled"
        }
        check(task.javaClass.name in listOf("org.gradle.api.tasks.compile.JavaCompile",
            "org.gradle.api.tasks.compile.JavaCompile_Decorated")) { "Unsupported Java recipe task type" }
        val o = task.options
        val explicitUtf8Jdk21 = profile.version == "9.6.1" &&
            task.sourceCompatibility == "21" && task.targetCompatibility == "21" && o.encoding == "UTF-8"
        val defaultUtf8Target17Jdk21 = profile.version == "9.6.1" &&
            task.sourceCompatibility == "17" && task.targetCompatibility == "17" &&
            o.release.orNull == null && o.encoding == null &&
            java.nio.charset.Charset.defaultCharset().name() == "UTF-8"
        val release8Utf8Jdk17 = profile.version == "7.4.2" && o.release.orNull == 8 &&
            task.sourceCompatibility == "1.8" && task.targetCompatibility == "1.8" &&
            o.encoding == null && java.nio.charset.Charset.defaultCharset().name() == "UTF-8"
        val explicitUtf8Jdk17 = profile.version == "7.5" && o.release.orNull == null &&
            task.sourceCompatibility == "1.8" && task.targetCompatibility == "1.8" && o.encoding == "UTF-8"
        // allJvmArgs invokes providers even when fork=false; reject before reading it.
        check(o.forkOptions.jvmArgumentProviders.isEmpty()) { "Java JVM argument providers are unhandled" }
        check(!o.isFork && o.forkOptions.executable == null && o.forkOptions.jvmArgs.isNullOrEmpty() &&
            o.forkOptions.allJvmArgs.isNullOrEmpty() && o.forkOptions.memoryInitialSize == null &&
            o.forkOptions.memoryMaximumSize == null && o.forkOptions.tempDir == null) { "Unsupported Java compiler fork policy" }
        check(o.compilerArgumentProviders.isEmpty()) { "Java compiler argument providers are unhandled" }
        check(o.compilerArgs.isEmpty() || o.compilerArgs == listOf("-proc:none")) { "Opaque Java compiler arguments are unhandled" }
        check(o.annotationProcessorPath?.files?.isEmpty() != false) { "Java annotation processing is unhandled" }
        check(o.sourcepath == null && o.bootstrapClasspath == null && o.extensionDirs == null &&
            (o.release.orNull == null || release8Utf8Jdk17) &&
            (o.encoding == null || explicitUtf8Jdk21 || explicitUtf8Jdk17)) { "Unsupported Java source/SDK/encoding policy" }
        check(o.javaModuleVersion.orNull == null && o.javaModuleMainClass.orNull == null &&
            task.source.files.none { it.name == "module-info.java" }) { "Java modules are unhandled" }
        fun invoke(name: String): Any = JavaCompile::class.java.getDeclaredMethod(name).apply {
            isAccessible = true
        }.invoke(task) ?: error("Java compiler adapter returned null: $name")
        val stableSources = invoke("getStableSources") as? FileCollection
            ?: error("Java stable source inventory is unavailable")
        check(stableSources.toList() == task.source.files.toList()) { "Java stable source order differs from the recorded task" }
        val selected = when (profile.version) {
            "7.4.2", "7.5" -> (invoke("getCompilerTool") as? Provider<*>)?.orNull as? org.gradle.jvm.toolchain.JavaCompiler
            "9.6.1" -> task.javaCompiler.orNull
            else -> error("Unsupported Java compiler selector")
        } ?: error("Selected Java compiler is unavailable")
        check(selected.javaClass.name == "org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler") {
            "Unsupported selected Java compiler"
        }
        val metadata = selected.metadata
        val home = metadata.installationPath.asFile
        check(metadata.languageVersion.asInt() == (if (explicitUtf8Jdk21 || defaultUtf8Target17Jdk21) 21 else 17) && home.isAbsolute && selectedHome != null &&
            home.canonicalFile == selectedHome.canonicalFile &&
            home.canonicalFile == java.io.File(System.getProperty("java.home")).canonicalFile) {
            "Java recipe requires the selected current SDK profile"
        }
        check(o.forkOptions.javaHome == null || o.forkOptions.javaHome?.canonicalFile == home.canonicalFile) {
            "Java compiler home selection differs"
        }
        val before = javaConfiguredState(task)
        val sourcepath = o.sourcepath
        val forkHome = o.forkOptions.javaHome
        val spec = try {
            invoke("createSpec")
        } finally {
            // The pinned createSpec mutates these two public options, even during metadata capture.
            o.sourcepath = sourcepath
            o.forkOptions.javaHome = forkHome
            check(before == javaConfiguredState(task)) { "Java compiler configuration was not restored" }
        }
        check(spec.javaClass.name == "org.gradle.api.internal.tasks.compile.DefaultJavaCompileSpec") {
            "Unsupported Java compiler execution spec"
        }
        val compileOptions = read(spec, "getCompileOptions") ?: error("Java compile options are unavailable")
        check((read(spec, "getModulePath") as? Collection<*>)?.isEmpty() == true &&
            (read(spec, "getAnnotationProcessorPath") as? Collection<*>)?.isEmpty() == true &&
            read(spec, "getRelease") == o.release.orNull && read(compileOptions, "getSourcepath") == null &&
            read(compileOptions, "getBootClasspath") == null && read(compileOptions, "getExtensionDirs") == null) {
            "Unsupported Java compiler effective inputs"
        }
        check(read(spec, "getCompileClasspath") == task.classpath.toList() &&
            read(spec, "getDestinationDir") == task.destinationDirectory.orNull?.asFile &&
            read(compileOptions, "getCompilerArgs") == o.allCompilerArgs.toList() &&
            read(compileOptions, "getEncoding") == o.encoding) {
            "Java effective compiler inputs differ from the recorded task"
        }
        val builderType = Class.forName("org.gradle.api.internal.tasks.compile.JavaCompilerArgumentsBuilder",
            false, JavaCompile::class.java.classLoader)
        val builder = builderType.constructors.single {
            it.parameterCount == 1 && it.parameterTypes[0].isInstance(spec)
        }.newInstance(spec)
        val arguments = read(builder, "build") as? List<*> ?: error("Java API compiler options are unavailable")
        check(arguments.all { it is String } && "-proc:none" in arguments) { "Unsupported Java API compiler options" }
        val runtimeVersion = read(metadata, "getJavaRuntimeVersion") as? String
            ?: error("Selected Java runtime version is unavailable")
        val gradleVersion = System.getProperty("java.version") ?: error("Gradle JVM version is unavailable")
        val fileEncoding = System.getProperty("file.encoding") ?: error("Gradle JVM encoding property is unavailable")
        check(runtimeVersion.isNotBlank() && gradleVersion.isNotBlank() && fileEncoding.isNotBlank()) {
            "Selected Java runtime defaults are empty"
        }
        return linkedMapOf(
            "adapter" to when {
                explicitUtf8Jdk21 -> "gradle-9.6.1-current-jdk21-utf8-javac-api-v1"
                defaultUtf8Target17Jdk21 -> "gradle-9.6.1-current-jdk21-target17-default-utf8-javac-api-v1"
                release8Utf8Jdk17 -> "gradle-7.4.2-current-jdk17-release8-utf8-javac-api-v1"
                explicitUtf8Jdk17 -> "gradle-7.5-current-jdk17-utf8-javac-api-v1"
                else -> "gradle-${profile.version}-current-jdk17-javac-api-v1"
            },
            "compiler_type" to selected.javaClass.name,
            "java_runtime_version" to runtimeVersion,
            "language_version" to metadata.languageVersion.toString(),
            "gradle_java_version" to gradleVersion,
            // file.encoding may differ from the JVM's already cached Charset.defaultCharset().
            "gradle_file_encoding" to fileEncoding,
            "effective_charset" to java.nio.charset.Charset.defaultCharset().name(),
            "locale" to java.util.Locale.getDefault().toLanguageTag(),
            "api_options" to arguments.filterIsInstance<String>(),
        )
    }

    // shortcut: pinned private action ABIs; an unknown layout never proves producer inactivity.
    fun producerActions(task: Task): Map<String, Any?>? = runCatching {
        val profile = javaGradleProfile(task)
        val loader = Task::class.java.classLoader
        val standard = Class.forName("org.gradle.api.internal.project.taskfactory.StandardTaskAction", false, loader)
        val incremental = Class.forName("org.gradle.api.internal.project.taskfactory.${profile.incrementalAction}", false, loader)
        val methodField = standard.getDeclaredField("method").apply { isAccessible = true }
        val rows = task.actions.map { action ->
            val method = if (action.javaClass == incremental)
                methodField.get(action) as? java.lang.reflect.Method else null
            linkedMapOf<String, Any?>(
                "class" to action.javaClass.name,
                "declaringClass" to method?.declaringClass?.name,
                "method" to method?.name,
                "parameterTypes" to method?.parameterTypes?.map { it.name },
            )
        }
        linkedMapOf("adapter" to "gradle-${profile.version}-stock-actions-v1", "actions" to rows)
    }.getOrNull()

    // shortcut: pinned stock Gradle predicates; only Gradle 7.4.2/KGP 1.8.21 KAPT is verified.
    // Read identity/bytecode, never evaluate a task predicate or a compiler/AP argument provider.
    private fun classEntrySha(type: Class<*>): String {
        val location = type.protectionDomain.codeSource?.location ?: error("Predicate origin unavailable")
        check(location.protocol == "file") { "Predicate origin is not a file" }
        val file = java.io.File(location.toURI())
        check(file.isAbsolute && file.isFile) { "Predicate origin is not an absolute JAR" }
        val bytes = java.util.jar.JarFile(file, false).use { jar ->
            val entry = jar.getJarEntry(type.name.replace('.', '/') + ".class") ?: error("Predicate class entry absent")
            check(entry.size in 1L..65536L) { "Predicate class entry size unhandled" }
            jar.getInputStream(entry).use { it.readBytes() }
        }
        return java.security.MessageDigest.getInstance("SHA-256").digest(bytes)
            .joinToString("") { "%02x".format(it.toInt() and 255) }
    }

    private fun stockKaptPredicate(predicate: Any): Boolean = runCatching {
        val wrapperName = "org.jetbrains.kotlin.gradle.tasks.configuration.KaptConfigKt\$sam\$org_gradle_api_specs_Spec\$0"
        if (predicate.javaClass.name != wrapperName) return@runCatching false
        val wrapper = predicate.javaClass
        check(classEntrySha(wrapper) == "17044b9583c5993e046245a18c8a811f803c557af3f3a229ac744d1736214885")
        val function = wrapper.getDeclaredField("function").apply { isAccessible = true }.get(predicate)
            ?: return@runCatching false
        val expected = Class.forName("org.jetbrains.kotlin.gradle.tasks.configuration.KaptConfig\$1\$1\$2", false, wrapper.classLoader)
        check(function.javaClass == expected && expected.protectionDomain.codeSource?.location == wrapper.protectionDomain.codeSource?.location)
        check(classEntrySha(expected) == "fbcbfb273fe96e173cf38d128f6cde565a7ac1e9045462aa4912d69590a9edd3")
        function === expected.getDeclaredField("INSTANCE").apply { isAccessible = true }.get(null)
    }.getOrDefault(false)

    fun producerOnlyIf(task: Task): Map<String, Any?>? = runCatching {
        val profile = javaGradleProfile(task)
        val loader = Task::class.java.classLoader
        val abstractTask = Class.forName("org.gradle.api.internal.AbstractTask", false, loader)
        val factory = abstractTask.getDeclaredMethod("createNewOnlyIfSpec").apply { isAccessible = true }
        val prototype = factory.invoke(task)
        val actual = abstractTask.getMethod("getOnlyIf").invoke(task)
        check(actual.javaClass == prototype.javaClass) { "Custom predicate composition" }
        val composite = Class.forName("org.gradle.api.specs.CompositeSpec", false, loader)
        val getSpecs = composite.getMethod("getSpecs")
        val defaults = getSpecs.invoke(prototype) as? List<*> ?: error("Stock predicate inventory unavailable")
        check(defaults.size == 1 && defaults[0] != null) { "Stock predicate inventory ambiguous" }
        val enabledType = defaults[0]!!.javaClass
        fun stockEnabledPredicate(predicate: Any): Boolean {
            if (predicate.javaClass != enabledType) return false
            if (profile.version in listOf("7.4.2", "7.5")) return true
            // Gradle 9.6.1 uses the same wrapper for stock and custom onlyIf predicates.
            check(enabledType.name == "org.gradle.api.internal.tasks.execution.SelfDescribingSpec") {
                "Unsupported stock predicate wrapper"
            }
            val field = enabledType.getDeclaredField("spec").apply { isAccessible = true }
            val stock = field.get(defaults[0]) ?: error("Stock predicate delegate unavailable")
            val delegate = field.get(predicate) ?: return false
            val source = stock.javaClass.protectionDomain.codeSource?.location
                ?: error("Stock predicate delegate origin unavailable")
            return delegate.javaClass == stock.javaClass &&
                delegate.javaClass.protectionDomain.codeSource?.location == source
        }
        val predicates = getSpecs.invoke(actual) as? List<*> ?: error("Predicate inventory unavailable")
        val rows = predicates.map { predicate ->
            check(predicate != null) { "Null task predicate" }
            linkedMapOf<String, Any?>(
                "class" to predicate.javaClass.name,
                "stockEnabledPredicate" to stockEnabledPredicate(predicate),
                "stockKaptPredicate" to (profile.version in listOf("7.4.2", "7.5") && stockKaptPredicate(predicate)),
            )
        }
        linkedMapOf("adapter" to "gradle-${profile.version}-stock-onlyif-v1", "predicates" to rows)
    }.getOrNull()


    private fun read(bean: Any, getter: String): Any? {
        val method = bean.javaClass.methods.firstOrNull { it.name == getter && it.parameterCount == 0 }
            ?: error("Compiler task does not expose $getter")
        val value = method.invoke(bean)
        return if (value is Provider<*>) value.orNull else value
    }

    // Public JVM compilation ownership, independent of names used by KAPT/KSP auxiliary tasks.
    // Missing/ambiguous adapters remain unknown rather than selecting the first Kotlin task.
    fun primaryKotlinCompileTask(project: Project, sourceSet: SourceSet): Task? {
        val extension = project.extensions.findByName("kotlin") ?: return null
        return runCatching {
            val target = read(extension, "getTarget") ?: error("Kotlin target is unavailable")
            val compilations = read(target, "getCompilations") as? Iterable<*>
                ?: error("Kotlin compilations are unavailable")
            val matching = compilations.filterNotNull().filter { compilation ->
                val getter = compilation.javaClass.methods.firstOrNull {
                    it.name == "getJavaSourceSet" && it.parameterCount == 0 &&
                        SourceSet::class.java.isAssignableFrom(it.returnType)
                }
                getter?.invoke(compilation) === sourceSet
            }
            if (matching.size != 1) return@runCatching null
            val task = read(matching.single(), "getCompileTaskProvider") as? Task
                ?: error("Kotlin compilation task is unavailable")
            val owner = task.project
            // Gradle 9 can expose distinct Project proxies for the same public project identity.
            check(owner.path == project.path && owner.projectDir == project.projectDir &&
                owner.rootProject.projectDir == project.rootProject.projectDir) {
                "Kotlin compilation task belongs to another project"
            }
            task
        }.getOrElse {
            project.logger.debug("ktlsp: primary Kotlin compilation is unavailable", it)
            null
        }
    }

    private fun kotlinClasspathSnapshotInputs(task: Task, recipe: Map<String, Any?>): Map<String, Any?>? = runCatching {
        if (task.project.gradle.gradleVersion != "9.6.1" || recipe["gradle_plugin_version"] != "2.4.0" || recipe["compiler_version"] != "2.4.0" ||
            recipe["run_via_build_tools_api"] != true || task.javaClass.name !in listOf(
                "org.jetbrains.kotlin.gradle.tasks.KotlinCompile",
                "org.jetbrains.kotlin.gradle.tasks.KotlinCompile_Decorated")) return@runCatching null
        // KGP 2.4.0 KotlinCompileConfig fills this @Classpath/@Incremental property from the
        // snapshot artifact view. BTA passes it only to snapshotBasedIcConfigurationBuilder;
        // the editor's fresh, nonincremental CLI never configures that operation.
        val owner = Class.forName("org.jetbrains.kotlin.gradle.tasks.KotlinCompile", false, task.javaClass.classLoader)
        val propertiesType = Class.forName("org.jetbrains.kotlin.gradle.tasks.KotlinCompile\$ClasspathSnapshotProperties",
            false, task.javaClass.classLoader)
        val getter = owner.getDeclaredMethod("getClasspathSnapshotProperties")
        check(getter.returnType == propertiesType && owner.isInstance(task)) { "Unsupported Kotlin snapshot getter" }
        val properties = getter.invoke(task) ?: error("Kotlin snapshot properties are unavailable")
        val snapshots = propertiesType.getDeclaredMethod("getClasspathSnapshot").invoke(properties) as? FileCollection
            ?: error("Kotlin classpath snapshot inputs are unavailable")
        val declared = task.inputs.files.files
        val files = snapshots.files
        check(files.all { it.isAbsolute && it in declared }) { "Kotlin snapshots are not declared file inputs" }
        // A plugin can register the same artifact as an additional compiler/config read.
        // Keep those roles: inspect registrations separately from KGP's annotated snapshot input.
        val visitorType = Class.forName("org.gradle.internal.properties.PropertyVisitor", false, Task::class.java.classLoader)
        val registered = linkedSetOf<java.io.File>()
        val visitor = java.lang.reflect.Proxy.newProxyInstance(visitorType.classLoader, arrayOf(visitorType)) { _, method, args ->
            when (method.name) {
                "visitInputFileProperty" -> {
                    check(method.parameterCount == 8 && method.parameterTypes[6].name == "org.gradle.internal.properties.PropertyValue") {
                        "Unsupported Gradle input file visitor"
                    }
                    val value = method.parameterTypes[6].getMethod("call").invoke(checkNotNull(args?.get(6)))
                    if (value != null) registered.addAll(task.project.files(value).files)
                    null
                }
                "toString" -> "ktlsp Gradle registered input inventory"
                "hashCode" -> 0
                "equals" -> false
                else -> null
            }
        }
        task.inputs.javaClass.getMethod("visitRegisteredProperties", visitorType).invoke(task.inputs, visitor)
        check(registered.all { it.isAbsolute }) { "Registered Kotlin input must be absolute" }
        linkedMapOf("adapter" to "gradle-9.6.1-kgp-2.4.0-classpath-snapshot-inputs-v1",
            "files" to files.map { it.absolutePath }.sorted(),
            "registered_inputs" to registered.map { it.absolutePath }.sorted())
    }.getOrNull()

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
        val versionHelper = Class.forName("org.jetbrains.kotlin.gradle.plugin.KotlinPluginWrapperKt", true, task.javaClass.classLoader)
        val gradlePluginVersion = versionHelper.getMethod("getKotlinPluginVersion", Project::class.java).invoke(null, task.project) as? String
            ?: error("Kotlin Gradle plugin version is unavailable")
        check(gradlePluginVersion.isNotBlank()) { "Kotlin Gradle plugin version is empty" }
        val buildToolsGetter = task.javaClass.methods.firstOrNull {
            it.name == "getRunViaBuildToolsApi\$kotlin_gradle_plugin_common" && it.parameterCount == 0
        }
        val runViaBuildToolsApi = if (buildToolsGetter != null) {
            read(task, "getRunViaBuildToolsApi\$kotlin_gradle_plugin_common") as? Boolean
                ?: error("Kotlin compiler Build Tools API policy is unavailable")
        } else {
            // Verified KGP 1.8.21: Tasks -> GradleCompilerEnvironment -> GradleCompilerRunner
            // passes defaultCompilerClasspath (+ currentJvmJdkToolsJar) directly to the worker.
            check(gradlePluginVersion == "1.8.21") { "Unverified legacy Kotlin compiler runtime: " + gradlePluginVersion }
            false
        }
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
            object : URLClassLoader((compilerClasspath + listOfNotNull(toolsJar)).map { it.toURI().toURL() }.toTypedArray(), null) {
                override fun getResourceAsStream(name: String): java.io.InputStream? {
                    val resource = getResource(name) ?: return null
                    return try {
                        resource.openConnection().apply { useCaches = false }.getInputStream()
                    } catch (_: java.io.IOException) {
                        null
                    }
                }
            }.use { loader ->
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
            "gradle_plugin_version" to gradlePluginVersion,
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
                record["source_files"] = task.source.files.map { it.absolutePath }
                record["classpath"] = task.classpath.files.map { it.absolutePath }

                val options = task.options
                record["arguments"] = options.allCompilerArgs.toList()
                record["annotation_processor_path"] = options.annotationProcessorPath?.files
                    ?.map { it.absolutePath } ?: emptyList<String>()
                val configuredCompiler = task.javaCompiler.orNull
                val defaultHome = if (configuredCompiler == null) defaultJavaHome(task) else null
                record["java_home"] = configuredCompiler?.metadata?.installationPath?.asFile?.absolutePath
                    ?: defaultHome?.absolutePath
                val javaOptions = linkedMapOf<String, Any?>(
                    "javaHomeSelection" to when {
                        configuredCompiler != null -> "javaCompiler"
                        defaultHome != null -> "gradle-current-jvm-${task.project.gradle.gradleVersion}"
                        else -> null
                    },
                    "sourceCompatibility" to task.sourceCompatibility,
                    "targetCompatibility" to task.targetCompatibility,
                    "release" to options.release.orNull,
                    "encoding" to options.encoding,
                    "bootstrapClasspath" to options.bootstrapClasspath?.files?.map { it.absolutePath },
                    "taskActionInventory" to producerActions(task),
                    "taskOnlyIfInventory" to producerOnlyIf(task),
                    "fork" to options.isFork,
                    "forkJavaHome" to options.forkOptions.javaHome?.absolutePath,
                    "forkExecutable" to options.forkOptions.executable,
                    "forkJvmArgs" to options.forkOptions.jvmArgs?.toList(),
                )
                javaOptions["javaCompilerRecipe"] = null
                javaOptions["javaCompilerRecipeProblem"] = null
                try {
                    javaOptions["javaCompilerRecipe"] = javaCompilerRecipe(task,
                        configuredCompiler?.metadata?.installationPath?.asFile ?: defaultHome)
                } catch (failure: Exception) {
                    // Recipe unavailability must not erase ordinary Java/Kotlin source ownership.
                    javaOptions["javaCompilerRecipeProblem"] = (failure.cause ?: failure).toString()
                } catch (failure: LinkageError) {
                    javaOptions["javaCompilerRecipeProblem"] = "Selected Java compiler adapter cannot be loaded: $failure"
                }
                record["options"] = javaOptions
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
                val recipe = kotlinCompilerRecipe(task, toolchain, javaHome)
                record["kotlin_compiler"] = recipe
                record["options"] = linkedMapOf(
                    "kotlinClasspathSnapshotInputs" to kotlinClasspathSnapshotInputs(task, recipe),
                    "taskActionInventory" to producerActions(task),
                    "taskOnlyIfInventory" to producerOnlyIf(task),
                )
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
                // Presence is observed without executing user provider code. Empty flattened
                // arguments never prove absence of a provider (including nested empty providers).
                val providerArguments = if (optionProviders.isEmpty()) emptyList<String>() else null
                record["options"] = linkedMapOf(
                    "pluginOptions" to pluginOptions,
                    "javacOptions" to read(task, "getJavacOptions"),
                    "processors" to read(task, "getAnnotationProcessorFqNames"),
                    "includeCompileClasspath" to read(task, "getIncludeCompileClasspath"),
                    "classpath" to (read(task, "getClasspath") as FileCollection).files.map { it.absolutePath },
                    "providerArguments" to providerArguments,
                    "annotationProcessorOptionProviderCount" to optionProviders.size,
                    "taskActionInventory" to producerActions(task),
                    "taskOnlyIfInventory" to producerOnlyIf(task),
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
        val sourceModules = linkedMapOf<String, Set<ModuleComponentIdentifier>>()
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
            sourceModules[sourceSet.name] = seen.filterIsInstance<ModuleComponentIdentifier>().toSet()
        }
        if (task.name == "ktlspDumpSources") {
            val parentXmlFactory by lazy {
                val factory = javax.xml.parsers.DocumentBuilderFactory.newInstance()
                factory.isNamespaceAware = true
                factory.setFeature(javax.xml.XMLConstants.FEATURE_SECURE_PROCESSING, true)
                factory.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true)
                factory.setFeature("http://xml.org/sax/features/external-general-entities", false)
                factory.setFeature("http://xml.org/sax/features/external-parameter-entities", false)
                factory.isXIncludeAware = false
                factory.isExpandEntityReferences = false
                // Older Gradle XML APIs lack these constants; the JAXP property names are stable.
                factory.setAttribute("http://javax.xml.XMLConstants/property/accessExternalDTD", "")
                factory.setAttribute("http://javax.xml.XMLConstants/property/accessExternalSchema", "")
                factory
            }
            fun parentCoordinate(file: java.io.File): Triple<String, String, String>? {
                require(file.length() in 1..(1L shl 20))
                val builder = parentXmlFactory.newDocumentBuilder()
                builder.setErrorHandler(org.xml.sax.helpers.DefaultHandler())
                val bytes = file.inputStream().use { it.readNBytes((1 shl 20) + 1) }
                require(bytes.size <= (1 shl 20))
                val root = builder.parse(java.io.ByteArrayInputStream(bytes)).documentElement
                val namespace = root.namespaceURI.orEmpty()
                require((root.localName ?: root.tagName) == "project" &&
                    namespace in listOf("", "http://maven.apache.org/POM/4.0.0"))
                fun direct(element: org.w3c.dom.Element, name: String): org.w3c.dom.Element? {
                    val children = element.childNodes
                    val matches = (0 until children.length).mapNotNull { children.item(it) as? org.w3c.dom.Element }
                        .filter { (it.localName ?: it.tagName) == name }
                    require(matches.size <= 1 && matches.all { it.namespaceURI.orEmpty() == namespace })
                    return matches.firstOrNull()
                }
                val parent = direct(root, "parent") ?: return null
                val values = listOf("groupId", "artifactId", "version").map { name ->
                    val value = direct(parent, name)
                    require(value != null && (0 until value.childNodes.length).none { value.childNodes.item(it) is org.w3c.dom.Element })
                    value.textContent.trim()
                }
                require(values.all { it !in listOf(".", "..") && it.matches(Regex("[A-Za-z0-9_.+\\-]+")) })
                return Triple(values[0], values[1], values[2])
            }
            fun <C : Component, A : Artifact> exportArtifacts(prefix: String, componentType: Class<C>, artifactType: Class<A>) {
                try {
                    val selected = sourceModules.values.flatten().toSet()
                    val query = if (selected.isEmpty()) null else task.project.dependencies.createArtifactResolutionQuery()
                        .forComponents(selected).withArtifacts(componentType, artifactType).execute()
                    val resolved = query?.resolvedComponents.orEmpty().toMutableList()
                    val membersByName = sourceModules.mapValues { (_, members) ->
                        mutableSetOf<ComponentIdentifier>().apply { addAll(members) }
                    }
                    val metadataFailures = mutableSetOf<String>()
                    if (prefix == "POM") {
                        val checked = mutableSetOf<Pair<String, ComponentIdentifier>>()
                        fun key(id: ComponentIdentifier): Triple<String, String, String>? =
                            (id as? ModuleComponentIdentifier)?.let { Triple(it.group, it.module, it.version) }
                        // Discovery only: Rust validates the exported GAVs, properties and encoding.
                        repeat(8) {
                            val parents = linkedMapOf<Triple<String, String, String>, MutableSet<String>>()
                            membersByName.forEach { (name, members) ->
                                resolved.filter { it.id in members && checked.add(name to it.id) }.forEach { component ->
                                    component.getArtifacts(artifactType).filterIsInstance<ResolvedArtifactResult>().forEach { artifact ->
                                        try {
                                            parentCoordinate(artifact.file)?.let { parents.getOrPut(it) { mutableSetOf() }.add(name) }
                                        } catch (failure: Exception) {
                                            if (failure is InterruptedException || failure is org.gradle.api.BuildCancelledException ||
                                                Thread.currentThread().isInterrupted) throw failure
                                            metadataFailures.add(name)
                                        }
                                    }
                                }
                            }
                            val missing = parents.keys.filter { parent -> resolved.none { key(it.id) == parent } }
                            if (missing.isNotEmpty()) {
                                val parentQuery = task.project.dependencies.createArtifactResolutionQuery()
                                    .withArtifacts(componentType, artifactType)
                                missing.forEach { parent -> parentQuery.forModule(parent.first, parent.second, parent.third) }
                                resolved.addAll(parentQuery.execute().resolvedComponents)
                            }
                            parents.forEach { (parent, owners) ->
                                val components = resolved.filter { key(it.id) == parent }
                                owners.forEach { name ->
                                    if (components.isEmpty()) metadataFailures.add(name)
                                    membersByName.getValue(name).addAll(components.map { it.id })
                                }
                            }
                        }
                        membersByName.forEach { (name, members) ->
                            if (members.any { (name to it) !in checked }) metadataFailures.add(name)
                        }
                    }
                    val failed = selected - resolved.map { it.id }.toSet()
                    membersByName.forEach { (name, members) ->
                        val components = resolved.filter { it.id in members }
                        val incomplete = name in metadataFailures || members.any { it in failed } || components.any { component ->
                            component.getArtifacts(artifactType).any { it !is ResolvedArtifactResult }
                        }
                        out.append("${prefix}QUERY\t${name}\t${if (incomplete) "failed" else "complete"}\n")
                        components.forEach componentLoop@ { component ->
                            val id = component.id as? ModuleComponentIdentifier ?: return@componentLoop
                            component.getArtifacts(artifactType).filterIsInstance<ResolvedArtifactResult>().forEach { artifact ->
                                out.append("${prefix}ARTIFACT\t${name}\t${artifact.file.absolutePath}\tMODULE\t${id.group}\t${id.module}\t${id.version}\n")
                            }
                        }
                    }
                } catch (failure: Exception) {
                    if (failure is InterruptedException || failure is org.gradle.api.BuildCancelledException ||
                        Thread.currentThread().isInterrupted) throw failure
                    sourceModules.keys.forEach { out.append("${prefix}QUERY\t${it}\tfailed\n") }
                    task.logger.info("ktlsp artifact metadata query unavailable")
                }
            }
            exportArtifacts("SOURCE", JvmLibrary::class.java, SourcesArtifact::class.java)
            exportArtifacts("POM", MavenModule::class.java, MavenPomArtifact::class.java)
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
//   KOTLIN_COMPILE_TASK\t<source set>\t<JSON public compilation task path or null>
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
                    val primaryKotlinTask = KtlspCompilerConfiguration.primaryKotlinCompileTask(project, sourceSet)
                    val kotlinTasks = (listOfNotNull(primaryKotlinTask) + listOf(
                        sourceSet.getCompileTaskName("kotlin"),
                        sourceSet.getTaskName("kapt", "Kotlin"),
                        sourceSet.getTaskName("kaptGenerateStubs", "Kotlin"),
                        sourceSet.getTaskName("ksp", "Kotlin"),
                    ).mapNotNull { taskName -> tasks.findByName(taskName) }).distinctBy { it.path }
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
                        listOf("KOTLIN_COMPILE_TASK\t${sourceSetName}\t${JsonOutput.toJson(primaryKotlinTask?.path)}") +
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

        listOf("ktlspDumpClasspath", "ktlspDumpSources").forEach { taskName ->
          tasks.register(taskName) {
            // Gradle 7 serializes enclosing Kotlin DSL closures, including their Project receiver.
            doLast(KtlspDumpClasspathAction(
                projectPath, projectDirPath, currentBuildRoot, includedBuildRoots,
                classpathFiles, resolutionRoot, sourceSets,
            ))
          }
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
