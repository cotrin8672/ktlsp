@file:OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class)
@file:Suppress("DEPRECATION")
package ktlsp.headers

import org.jetbrains.kotlin.com.intellij.mock.MockProject
import org.jetbrains.kotlin.com.intellij.psi.PsiErrorElement
import org.jetbrains.kotlin.com.intellij.psi.PsiRecursiveElementWalkingVisitor
import org.jetbrains.kotlin.compiler.plugin.ComponentRegistrar
import org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar
import org.jetbrains.kotlin.cli.common.CLIConfigurationKeys
import org.jetbrains.kotlin.cli.jvm.config.JvmClasspathRoot
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.extensions.ProjectExtensionDescriptor
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.resolve.BindingContext
import org.jetbrains.kotlin.diagnostics.Severity
import org.jetbrains.org.objectweb.asm.ClassReader
import java.io.File
import java.security.MessageDigest

/** An external naming-domain experiment. API status and producer authority remain separate. */
internal class PackageDomainProof(
    private val configuration: CompilerConfiguration,
    private val project: MockProject,
    private val context: BindingContext,
    private val files: Collection<KtFile>,
    pluginInventory: File,
    selectedClasspath: File,
) {
    private fun key(file: File) = file.canonicalPath.replace('\\', '/').lowercase()
    private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    private val allowedInputs = pluginInventory.readLines().associate { line ->
        val parts = line.split('\t')
        check(parts.size == 2 && parts[0].length == 64) { "Malformed pinned plugin/runtime inventory" }
        val file = File(parts[1]).canonicalFile
        check(file.isFile && sha(file.readBytes()) == parts[0]) { "Pinned plugin/runtime changed: $file" }
        key(file) to parts[0]
    }
    private val globalIssues = mutableListOf<String>()
    private val issues = files.associate { key(File(it.virtualFilePath)) to mutableListOf<String>() }
    private val packages = files.associate { key(File(it.virtualFilePath)) to it.packageFqName.asString() }
    private val inspected = mutableListOf<Map<String, Any?>>()
    private val represented = mutableSetOf<String>()
    val facts = linkedMapOf<String, Any?>(
        "schema" to 1, "stageCompleted" to false, "allPackageDomainsKnown" to false,
        "nameDomainIsWholePackage" to true, "apiAuthority" to false, "producerClosure" to false,
        "globalIssues" to globalIssues, "sourceDomains" to emptyList<Any>(), "rawClasses" to inspected,
    )

    private fun origin(clazz: Class<*>): Map<String, Any?> {
        return try {
            val file = File(clazz.protectionDomain.codeSource.location.toURI()).canonicalFile
            val digest = if (file.isFile) sha(file.readBytes()) else null
            mapOf("class" to clazz.name, "path" to file.path, "sha256" to digest,
                "pinnedSelectedInput" to (digest != null && allowedInputs[key(file)] == digest))
        } catch (e: Exception) {
            mapOf("class" to clazz.name, "pinnedSelectedInput" to false,
                "originFailure" to (e.javaClass.name + ": " + e.message))
        }
    }

    private fun hook(className: String): List<Class<*>> {
        val type = Class.forName(className, false, ComponentRegistrar::class.java.classLoader)
        val descriptor = type.getField("Companion").get(null) as ProjectExtensionDescriptor<*>
        return descriptor.getInstances(project).map { it.javaClass }
    }

    init {
        // Stock scripting is admitted only for exact pinned inputs and non-script, unchanged PSI.
        // Its source-only naming effects are audited in primary-sources.json; unknown hooks deny the domain.
        val allowedHooks = mapOf(
            "org.jetbrains.kotlin.resolve.extensions.SyntheticResolveExtension" to setOf("org.jetbrains.kotlin.scripting.extensions.ScriptingResolveExtension"),
            "org.jetbrains.kotlin.resolve.extensions.ExtraImportsProviderExtension" to setOf("org.jetbrains.kotlin.scripting.extensions.ScriptExtraImportsProviderExtension"),
            "org.jetbrains.kotlin.extensions.CompilerConfigurationExtension" to setOf("org.jetbrains.kotlin.scripting.compiler.plugin.ScriptingCompilerConfigurationExtension"),
            "org.jetbrains.kotlin.extensions.CollectAdditionalSourcesExtension" to setOf("org.jetbrains.kotlin.scripting.compiler.plugin.extensions.ScriptingCollectAdditionalSourcesExtension"),
            "org.jetbrains.kotlin.extensions.ProcessSourcesBeforeCompilingExtension" to setOf("org.jetbrains.kotlin.scripting.compiler.plugin.extensions.ScriptingProcessSourcesBeforeCompilingExtension"),
        )
        val deniedHooks = listOf(
            "org.jetbrains.kotlin.resolve.jvm.extensions.PackageFragmentProviderExtension",
            "org.jetbrains.kotlin.resolve.jvm.extensions.SyntheticJavaResolveExtension",
            "org.jetbrains.kotlin.synthetic.SyntheticScopeProviderExtension",
            "org.jetbrains.kotlin.extensions.StorageComponentContainerContributor",
            "org.jetbrains.kotlin.extensions.DeclarationAttributeAltererExtension",
            "org.jetbrains.kotlin.extensions.PreprocessedVirtualFileFactoryExtension",
            "org.jetbrains.kotlin.extensions.TypeAttributeTranslatorExtension",
            "org.jetbrains.kotlin.extensions.internal.CandidateInterceptor",
            "org.jetbrains.kotlin.extensions.internal.TypeResolutionInterceptor",
            "org.jetbrains.kotlin.resolve.extensions.AssignResolutionAltererExtension",
            "org.jetbrains.kotlin.codegen.extensions.ExpressionCodegenExtension",
        )
        val hooks = linkedMapOf<String, Any?>()
        for (name in allowedHooks.keys + deniedHooks) {
            try {
                val entries = hook(name).map(::origin)
                hooks[name] = entries
                if (entries.any { it["class"] !in allowedHooks[name].orEmpty() || it["pinnedSelectedInput"] != true }) {
                    globalIssues.add("Unproved declaration/source hook: $name")
                }
            } catch (e: Exception) {
                hooks[name] = mapOf("inventoryFailure" to (e.javaClass.name + ": " + e.message))
                globalIssues.add("Hook inventory unavailable: $name")
            }
        }
        facts["hookInventory"] = hooks
        val allowedRegistrars = setOf(
            "org.jetbrains.kotlin.scripting.compiler.plugin.ScriptingCompilerConfigurationComponentRegistrar",
            "org.jetbrains.kotlin.scripting.compiler.plugin.ScriptingK2CompilerPluginRegistrar",
            "org.jetbrains.kotlin.kapt3.Kapt3ComponentRegistrar",
            "com.google.devtools.ksp.KotlinSymbolProcessingComponentRegistrar",
        )
        val registrars = (configuration.getList(ComponentRegistrar.PLUGIN_COMPONENT_REGISTRARS).map { it.javaClass } +
            configuration.getList(CompilerPluginRegistrar.COMPILER_PLUGIN_REGISTRARS).map { it.javaClass })
            .map { type -> if (type === HeaderRegistrar::class.java) mapOf("class" to type.name, "toolOwned" to true)
                else origin(type) }
        facts["registrars"] = registrars
        if (registrars.any { it["toolOwned"] != true && (it["class"] !in allowedRegistrars || it["pinnedSelectedInput"] != true) }) {
            globalIssues.add("Unexplained registrar contribution")
        }
        val expectedClasspath = selectedClasspath.readLines().map { key(File(it)) }
        val actualClasspath = configuration.getList(CLIConfigurationKeys.CONTENT_ROOTS)
            .filterIsInstance<JvmClasspathRoot>().map { key(it.file) }
        facts["requestedClasspath"] = expectedClasspath
        facts["effectiveClasspath"] = actualClasspath
        if (actualClasspath != expectedClasspath) globalIssues.add("Effective classpath differs from selected ordered classpath")
        for (file in files) {
            val problems = issues.getValue(key(File(file.virtualFilePath)))
            if (!file.name.endsWith(".kt", ignoreCase = true)) problems.add("Only ordinary .kt sources are admitted")
            if (file.isScript()) problems.add("Script source has no admitted package domain")
            if (file.fileAnnotationList?.annotationEntries.orEmpty().isNotEmpty()) problems.add("File annotations require JVM naming proof")
            val directive = file.packageDirective
            if (directive == null || directive.fqName.isRoot) problems.add("Only an explicit non-root package is admitted")
            if (file.packageFqName.pathSegments().any { !Name.isValidIdentifier(it.asString()) }) problems.add("Invalid package component")
            file.accept(object : PsiRecursiveElementWalkingVisitor() {
                override fun visitErrorElement(element: PsiErrorElement) {
                    problems.add("PSI syntax error at " + element.textRange.startOffset)
                    super.visitErrorElement(element)
                }
            })
            if (directive != null && context.diagnostics.all().any { d -> d.psiFile === file && d.severity == Severity.ERROR &&
                d.textRanges.any { it.intersects(directive.textRange) } }) problems.add("Package/header diagnostic")
            if (context[BindingContext.PACKAGE_TO_FILES, file.packageFqName]?.contains(file) != true) problems.add("Frontend package/file binding unavailable")
        }
        refresh()
    }

    fun inspectClass(relative: String, sources: List<String>, bytes: ByteArray) {
        val name = ClassReader(bytes).className
        val packageName = name.substringBeforeLast('/', "").replace('/', '.')
        val reasons = mutableListOf<String>()
        if (relative.replace('\\', '/') != name + ".class") reasons.add("ASM internal name differs from output path")
        if (sources.size != 1 || sources.any { it !in packages }) reasons.add("Class needs exactly one accounted selected source")
        for (source in sources) {
            represented.add(source)
            if (packages[source] != packageName) {
                issues[source]?.add("Emitted class leaves source package: $name")
                reasons.add("Emitted class leaves source package")
            }
        }
        if (reasons.isNotEmpty()) globalIssues.addAll(reasons)
        inspected.add(mapOf("relativePath" to relative, "internalName" to name, "package" to packageName,
            "sources" to sources, "sha256" to sha(bytes), "issues" to reasons))
        refresh()
    }

    private fun refresh() {
        facts["sourceDomains"] = files.map { file ->
            val source = key(File(file.virtualFilePath))
            mapOf("source" to source, "package" to packages.getValue(source), "issues" to issues.getValue(source),
                "status" to if (globalIssues.isEmpty() && issues.getValue(source).isEmpty() && facts["stageCompleted"] == true) "KnownWholePackage" else "Unknown")
        }
    }

    fun finish() {
        if (represented != packages.keys) globalIssues.add("Raw class origin coverage differs from selected Kotlin sources")
        facts["stageCompleted"] = true
        facts["allPackageDomainsKnown"] = globalIssues.isEmpty() && issues.values.all { it.isEmpty() }
        refresh()
    }

    fun abort() {
        facts["stageCompleted"] = false
        facts["allPackageDomainsKnown"] = false
        refresh()
    }
}
