@file:OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class)

package ktlsp.headers

import java.io.File
import java.security.MessageDigest
import org.jetbrains.kotlin.cli.common.CLIConfigurationKeys
import org.jetbrains.kotlin.cli.jvm.config.JavaSourceRoot
import org.jetbrains.kotlin.cli.jvm.config.JvmClasspathRoot
import org.jetbrains.kotlin.codegen.ClassFileFactory
import org.jetbrains.kotlin.codegen.extensions.ClassFileFactoryFinalizerExtension
import org.jetbrains.kotlin.compiler.plugin.*
import org.jetbrains.kotlin.config.*
import org.jetbrains.kotlin.diagnostics.DiagnosticReporter
import org.jetbrains.kotlin.extensions.ProjectExtensionDescriptor
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.analysis.checkers.MppCheckerKind
import org.jetbrains.kotlin.fir.analysis.checkers.context.CheckerContext
import org.jetbrains.kotlin.fir.analysis.checkers.declaration.DeclarationCheckers
import org.jetbrains.kotlin.fir.analysis.checkers.declaration.FirDeclarationChecker
import org.jetbrains.kotlin.fir.analysis.extensions.FirAdditionalCheckersExtension
import org.jetbrains.kotlin.fir.declarations.FirFile
import org.jetbrains.kotlin.fir.extensions.FirExtensionRegistrar
import org.jetbrains.kotlin.fir.extensions.FirExtensionRegistrarAdapter
import org.jetbrains.kotlin.fir.extensions.extensionService
import org.jetbrains.kotlin.name.Name
import org.jetbrains.org.objectweb.asm.ClassReader

private val KEYS = listOf("output", "receipt", "inventory", "plugins", "classpath")
    .associateWith { CompilerConfigurationKey.create<String>("ktlsp K2 complete $it") }
private fun key(file: File) = file.canonicalPath.replace('\\', '/').lowercase()
private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes)
    .joinToString("") { "%02x".format(it.toInt() and 255) }
private fun json(v: Any?): String = when (v) {
    null -> "null"
    is Boolean, is Number -> v.toString()
    is Map<*, *> -> v.entries.joinToString(",", "{", "}") { json(it.key.toString()) + ":" + json(it.value) }
    is Iterable<*> -> v.joinToString(",", "[", "]") { json(it) }
    else -> "\"" + buildString {
        for (c in v.toString()) when (c) {
            '"' -> append("\\\"")
            '\\' -> append("\\\\")
            '\n' -> append("\\n")
            '\r' -> append("\\r")
            '\t' -> append("\\t")
            else -> if (c.code < 32) append("\\u%04x".format(c.code)) else append(c)
        }
    } + "\""
}

class HeaderOptions : CommandLineProcessor {
    override val pluginId = "ktlsp.current-api-headers"
    override val pluginOptions = KEYS.keys.map {
        CliOption(it, "absolute-path", "Owned complete-output input/output", required = true, allowMultipleOccurrences = false)
    }
    override fun processOption(option: AbstractCliOption, value: String, configuration: CompilerConfiguration) {
        configuration.put(KEYS.getValue(option.optionName), value)
    }
}

class HeaderRegistrar : CompilerPluginRegistrar() {
    override val pluginId = "ktlsp.current-api-headers"
    override val supportsK2 = true
    override fun ExtensionStorage.registerExtensions(configuration: CompilerConfiguration) {
        check(KotlinCompilerVersion.VERSION == "2.4.0")
        val proof = CompleteDomain(configuration)
        FirExtensionRegistrarAdapter.registerExtension(object : FirExtensionRegistrar() {
            override fun ExtensionRegistrarContext.configurePlugin() {
                val factory: (FirSession) -> FirAdditionalCheckersExtension = { session -> DomainCheckers(session, proof) }
                +factory
            }
        })
        ClassFileFactoryFinalizerExtension.registerExtension(object : ClassFileFactoryFinalizerExtension {
            override fun finalizeClassFactory(factory: ClassFileFactory) = proof.finish(factory)
        })
    }
}

private class DomainCheckers(session: FirSession, private val proof: CompleteDomain) : FirAdditionalCheckersExtension(session) {
    override val declarationCheckers = object : DeclarationCheckers() {
        override val fileCheckers: Set<FirDeclarationChecker<FirFile>> = setOf(
            object : FirDeclarationChecker<FirFile>(MppCheckerKind.Common) {
                context(context: CheckerContext, reporter: DiagnosticReporter)
                override fun check(declaration: FirFile) {
                    val service = session.extensionService
                    val extensions = service.javaClass.getMethod("getAllExtensions").invoke(service) as? List<*>
                        ?: error("FIR extension inventory unavailable")
                    proof.file(declaration, extensions)
                }
            }
        )
    }
}

/** Successful compilation is certified only by the Rust owner after exit/EOF and disk checks. */
private class CompleteDomain(private val configuration: CompilerConfiguration) {
    private val receipt = File(configuration.getNotNull(KEYS.getValue("receipt"))).canonicalFile
    private val out = File(configuration.getNotNull(KEYS.getValue("output"))).canonicalFile
    private val selected = File(configuration.getNotNull(KEYS.getValue("inventory"))).readLines().map {
        val row = it.split('\t')
        check(row.size == 3 && row[0] in listOf("kotlin", "java"))
        Triple(row[0], row[1], File(row[2]).canonicalFile)
    }
    private val kotlin = selected.filter { it.first == "kotlin" }.associateBy { key(it.third) }
    private val pinned = File(configuration.getNotNull(KEYS.getValue("plugins"))).readLines().associate {
        val row = it.split('\t')
        check(row.size == 2)
        key(File(row[1])) to (row[0] to File(row[1]).canonicalFile)
    }
    private val domains = linkedMapOf<String, Map<String, Any?>>()
    private val issues = mutableListOf<String>()
    private val firHooks = linkedSetOf<String>()
    private var callbacks = 0

    init {
        check(selected.isNotEmpty() && selected.map { key(it.third) }.toSet().size == selected.size)
        check(!receipt.exists() && out.isDirectory && out.listFiles()!!.isEmpty())
        stable()
        check(pinned.values.all { (digest, file) -> file.isFile && sha(file.readBytes()) == digest })
    }

    private fun stable() = selected.forEach { check(sha(it.third.readBytes()) == it.second) }

    @OptIn(org.jetbrains.kotlin.fir.declarations.DirectDeclarationsAccess::class)
    @Synchronized fun file(file: FirFile, extensions: List<*>) {
        val path = key(File(checkNotNull(file.sourceFile?.path)))
        check(path in kotlin && path !in domains)
        val selected = kotlin.getValue(path)
        check(sha(selected.third.readBytes()) == selected.second)
        val pkg = file.packageDirective.packageFqName
        val local = mutableListOf<String>()
        if (!path.endsWith(".kt") || pkg.isRoot || file.packageDirective.source == null ||
            pkg.pathSegments().any { !Name.isValidIdentifier(it.asString()) }) local.add("Ordinary explicit package required")
        extensions.forEach {
            check(it != null)
            firHooks.add(it.javaClass.name)
            // Full normal outputs are compared with the original canonical invocation by Rust.
        }
        domains[path] = mapOf("source" to path, "package" to pkg.asString(), "originalFir" to true,
            "sourceSha256" to selected.second, "declarationCount" to file.declarations.size, "issues" to local,
            "status" to if (local.isEmpty()) "KnownWholePackage" else "Unknown")
    }

    @Synchronized fun finish(factory: ClassFileFactory) {
        stable()
        check(++callbacks == 1 && !receipt.exists())
        val project = factory.generationState.project
        fun hooks(name: String): List<Any> {
            val type = Class.forName(name, false, HeaderRegistrar::class.java.classLoader)
            val descriptor = type.getField("Companion").get(null) as ProjectExtensionDescriptor<*>
            return descriptor.getInstances(project).map { it as Any }
        }
        val hookInventory = linkedMapOf<String, List<String>>()
        @Suppress("DEPRECATION", "DEPRECATION_ERROR")
        val registrars = configuration.getList(ComponentRegistrar.PLUGIN_COMPONENT_REGISTRARS).map { it.javaClass } +
            configuration.getList(CompilerPluginRegistrar.COMPILER_PLUGIN_REGISTRARS).map { it.javaClass }
        val registrarInventory = registrars.map { type ->
            if (type === HeaderRegistrar::class.java) mapOf("class" to type.name, "toolOwned" to true)
            else {
                val file = File(type.protectionDomain.codeSource.location.toURI()).canonicalFile
                val digest = if (file.isFile) sha(file.readBytes()) else null
                mapOf("class" to type.name, "path" to file.path, "sha256" to digest,
                    "pinnedSelectedInput" to (digest != null && pinned[key(file)]?.first == digest))
            }
        }
        for (name in listOf(
            "org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension",
            "org.jetbrains.kotlin.backend.jvm.extensions.ClassGeneratorExtension",
            "org.jetbrains.kotlin.codegen.extensions.ClassFileFactoryFinalizerExtension",
            "org.jetbrains.kotlin.extensions.CollectAdditionalSourcesExtension",
            "org.jetbrains.kotlin.extensions.ProcessSourcesBeforeCompilingExtension",
            "org.jetbrains.kotlin.extensions.PreprocessedVirtualFileFactoryExtension",
        )) {
            try {
                val entries = hooks(name)
                hookInventory[name] = entries.map { it.javaClass.name }
            } catch (e: Exception) {
                hookInventory[name] = listOf("inventory-failure:" + e.javaClass.name + ":" + e.message)
            }
        }
        val cp = configuration.getList(CLIConfigurationKeys.CONTENT_ROOTS).filterIsInstance<JvmClasspathRoot>().map { key(it.file) }
        val expectedCp = File(configuration.getNotNull(KEYS.getValue("classpath"))).readLines().map { key(File(it)) }
        if (cp != expectedCp) issues.add("Effective classpath differs")
        val javaRoots = configuration.getList(CLIConfigurationKeys.CONTENT_ROOTS).filterIsInstance<JavaSourceRoot>().map { key(it.file) }
        if (javaRoots.toSet() != selected.filter { it.first == "java" }.map { key(it.third) }.toSet() ||
            javaRoots.size != javaRoots.toSet().size) issues.add("Java source membership differs")
        if (domains.keys != kotlin.keys) issues.add("Original FIR source membership differs")
        val classes = mutableListOf<Map<String, Any?>>()
        val represented = mutableSetOf<String>()
        var metadataCount = 0
        val outputs = factory.asList().map { output ->
            val bytes = output.asByteArray()
            val sources = output.sourceFiles.map(::key)
            val relative = output.relativePath.replace('\\', '/')
            val role = if (relative.endsWith(".class")) {
                val internal = ClassReader(bytes).className
                val pkg = internal.substringBeforeLast('/', "").replace('/', '.')
                val local = mutableListOf<String>()
                if (relative != "$internal.class" || sources.size != 1 || sources.any { it !in kotlin }) local.add("Class origin/name unaccounted")
                if (sources.any { domains[it]?.get("package") != pkg }) local.add("Class leaves original source package")
                represented.addAll(sources)
                issues.addAll(local)
                classes.add(mapOf("relativePath" to relative, "internalName" to internal, "package" to pkg,
                    "sources" to sources, "sha256" to sha(bytes), "issues" to local))
                "k2-complete-class"
            } else {
                if (!relative.matches(Regex("META-INF/[A-Za-z0-9_.-]+\\.kotlin_module")) ||
                    sources.any { it !in kotlin } || ++metadataCount != 1) issues.add("Metadata output role unproved")
                "k2-module-metadata"
            }
            mapOf("relativePath" to relative, "bytes" to bytes.size, "sha256" to sha(bytes),
                "sources" to sources, "role" to role, "apiStatus" to "CompleteCompilation")
        }
        val unrepresented = kotlin.keys - represented
        if (!represented.all { it in kotlin } || unrepresented.any { domains[it]?.get("declarationCount") != 0 }) {
            issues.add("Class/source coverage differs")
        }
        val allKnown = issues.isEmpty() && domains.values.all { (it["issues"] as List<*>).isEmpty() }
        val facts = mapOf("schema" to 2, "component" to "k2-complete-output", "compilerVersion" to KotlinCompilerVersion.VERSION,
            "stageCompleted" to true, "callbackCount" to callbacks, "factoryDone" to true,
            "sourceInventory" to selected.map { mapOf("kind" to it.first, "sha256" to it.second, "path" to it.third.path) },
            "allSelectedSourcesIncluded" to (domains.keys == kotlin.keys), "outputs" to outputs,
            "gradleTaskReplay" to false, "producerClosure" to false, "abiParityProven" to false,
            "packageDomainProof" to mapOf("schema" to 2, "basis" to "original-fir-complete-output",
                "stageCompleted" to true, "allPackageDomainsKnown" to allKnown, "nameDomainIsWholePackage" to true,
                "globalIssues" to issues, "sourceDomains" to domains.values.toList(), "rawClasses" to classes,
                "firExtensions" to firHooks.toList(), "hookInventory" to hookInventory,
                "registrars" to registrarInventory,
                "requestedClasspath" to expectedCp, "effectiveClasspath" to cp,
                "apiAuthority" to false, "producerClosure" to false))
        stable()
        receipt.writeText(json(facts))
    }
}
