@file:Suppress("DEPRECATION")
@file:OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class)

package ktlsp.positive

import java.io.File
import java.security.MessageDigest
import org.jetbrains.kotlin.com.intellij.mock.MockProject
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.analyzer.AnalysisResult
import org.jetbrains.kotlin.backend.jvm.JvmIrCodegenFactory
import org.jetbrains.kotlin.cli.common.CLIConfigurationKeys
import org.jetbrains.kotlin.codegen.ClassBuilderMode
import org.jetbrains.kotlin.codegen.KotlinCodegenFacade
import org.jetbrains.kotlin.codegen.OriginCollectingClassBuilderFactory
import org.jetbrains.kotlin.codegen.state.GenerationState
import org.jetbrains.kotlin.compiler.plugin.*
import org.jetbrains.kotlin.config.*
import org.jetbrains.kotlin.descriptors.*
import org.jetbrains.kotlin.diagnostics.DiagnosticReporterFactory
import org.jetbrains.kotlin.diagnostics.Severity
import org.jetbrains.kotlin.load.kotlin.computeJvmDescriptor
import org.jetbrains.kotlin.psi.*
import org.jetbrains.kotlin.resolve.BindingContext
import org.jetbrains.kotlin.resolve.BindingTrace
import org.jetbrains.kotlin.resolve.DescriptorUtils
import org.jetbrains.kotlin.resolve.jvm.extensions.AnalysisHandlerExtension
import org.jetbrains.kotlin.types.error.ErrorUtils

private val OUTPUT = CompilerConfigurationKey.create<String>("ktlsp positive headers")
private val RECEIPT = CompilerConfigurationKey.create<String>("ktlsp positive receipt")
private val INVENTORY = CompilerConfigurationKey.create<String>("ktlsp exact sources")
private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
private fun key(file: File) = file.canonicalPath.replace('\\', '/').lowercase()
private fun signaturePreserves(type: org.jetbrains.kotlin.types.KotlinType): Boolean =
    !ErrorUtils.containsErrorType(type) && !org.jetbrains.kotlin.types.TypeUtils.isNullableType(type) &&
        type.unwrap() !is org.jetbrains.kotlin.types.FlexibleType &&
        type.arguments.all { !it.isStarProjection && signaturePreserves(it.type) }
private fun json(v: Any?): String = when (v) {
    null -> "null"
    is Boolean, is Number -> v.toString()
    is Map<*, *> -> v.entries.joinToString(",", "{", "}") { json(it.key.toString()) + ":" + json(it.value) }
    is Iterable<*> -> v.joinToString(",", "[", "]") { json(it) }
    else -> "\"" + v.toString().map { c -> when(c) {
        '"' -> "\\\""; '\\' -> "\\\\"; '\n' -> "\\n"; '\r' -> "\\r"; '\t' -> "\\t"
        else -> if (c.code < 32) "\\u%04x".format(c.code) else c.toString()
    } }.joinToString("") + "\""
}

class PositiveOptions : CommandLineProcessor {
    override val pluginId = "ktlsp.positive-headers"
    override val pluginOptions = listOf("output", "receipt", "inventory").map {
        CliOption(it, "absolute-path", "Tool-owned positive observation", required = true, allowMultipleOccurrences = false)
    }
    override fun processOption(option: AbstractCliOption, value: String, configuration: CompilerConfiguration) {
        configuration.put(when(option.optionName) {
            "output" -> OUTPUT; "receipt" -> RECEIPT; "inventory" -> INVENTORY
            else -> error("Unknown positive observation option")
        }, value)
    }
}

class PositiveRegistrar : ComponentRegistrar {
    override fun registerProjectComponents(project: MockProject, configuration: CompilerConfiguration) {
        AnalysisHandlerExtension.registerExtension(project, PositiveExtension(configuration))
    }
}

private class PositiveExtension(private val original: CompilerConfiguration) : AnalysisHandlerExtension {
    private var calls = 0
    override fun analysisCompleted(project: Project, module: ModuleDescriptor, bindingTrace: BindingTrace,
        files: Collection<KtFile>): AnalysisResult? {
        val context = bindingTrace.bindingContext
        val receipt = File(original.getNotNull(RECEIPT))
        val out = File(original.getNotNull(OUTPUT)).canonicalFile
        fun diagnostics() = context.diagnostics.all().map { d ->
            listOf(d.factory.name, d.severity.name, key(File(d.psiFile.virtualFile.path)),
                d.textRanges.joinToString(";") { "${it.startOffset}:${it.endOffset}" }).joinToString("|")
        }.sorted()
        val before = diagnostics()
        val facts = linkedMapOf<String, Any?>("schema" to 1, "compilerVersion" to KotlinCompilerVersion.getVersion(),
            "positiveOnly" to true, "callbackCount" to ++calls, "stageCompleted" to false)
        try {
            check(calls == 1 && !receipt.exists())
            check(KotlinCompilerVersion.getVersion() == "1.9.23")
            check(!original.getBoolean(CommonConfigurationKeys.USE_FIR) && original.getBoolean(JVMConfigurationKeys.IR))
            check(out.isDirectory && out.listFiles()!!.isEmpty())
            val expected = File(original.getNotNull(INVENTORY)).readLines().associate { line ->
                val parts = line.split('\t'); check(parts.size == 2)
                key(File(parts[1])) to parts[0]
            }
            check(files.map { key(File(it.virtualFile.path)) }.toSet() == expected.keys)
            files.forEach { check(sha(it.text.toByteArray(Charsets.UTF_8)) == expected.getValue(key(File(it.virtualFile.path)))) }
            val errors = context.diagnostics.all().filter { it.severity == Severity.ERROR }
            val producers = files.filter { file -> errors.none { it.psiFile === file } &&
                !org.jetbrains.kotlin.com.intellij.psi.util.PsiTreeUtil.hasErrorElements(file) }
            val classes = mutableListOf<Map<String, Any?>>()
            for (file in files) {
                // An erroneous producer cannot certify its API; errors in a consumer retain their original diagnostics.
                if (errors.any { it.psiFile === file }) continue
                fun observe(owner: ClassDescriptor) {
                    if (owner.visibility != DescriptorVisibilities.PUBLIC || ErrorUtils.isError(owner)) return
                    // Signature omits Kotlin nullability; nullable surfaces remain unknown until normalized.
                    val methods = owner.unsubstitutedMemberScope.getContributedDescriptors()
                        .filterIsInstance<SimpleFunctionDescriptor>().filter {
                            it.visibility == DescriptorVisibilities.PUBLIC && it.kind == CallableMemberDescriptor.Kind.SYNTHESIZED &&
                                it.returnType?.let(::signaturePreserves) == true &&
                                it.valueParameters.all { parameter -> signaturePreserves(parameter.type) }
                        }.map { it.computeJvmDescriptor() }.sorted()
                    classes.add(mapOf("owner" to DescriptorUtils.getFqNameSafe(owner).asString(), "companion" to owner.isCompanionObject,
                        "source" to key(File(file.virtualFile.path)), "methods" to methods))
                    owner.companionObjectDescriptor?.let(::observe)
                }
                file.declarations.filterIsInstance<KtClassOrObject>().forEach { declaration ->
                    context[BindingContext.CLASS, declaration]?.let(::observe)
                }
            }
            facts["classes"] = classes
            val configuration = original.copy().apply { put(JVMConfigurationKeys.DO_NOT_CLEAR_BINDING_CONTEXT, true) }
            fun generate(mode: ClassBuilderMode, selected: Collection<KtFile>): List<Triple<String, List<String>, ByteArray>> {
                val reporter = DiagnosticReporterFactory.createReporter()
                val state = GenerationState.Builder(project, OriginCollectingClassBuilderFactory(mode), module, context, configuration)
                    .isIrBackend(true).ignoreErrors(true).diagnosticReporter(reporter).build()
                return try {
                    KotlinCodegenFacade.compileCorrectFiles(selected, state,
                        JvmIrCodegenFactory(configuration, configuration.get(CLIConfigurationKeys.PHASE_CONFIG)))
                    check(!reporter.hasErrors && state.collectedExtraJvmDiagnostics.all().none { it.severity == Severity.ERROR })
                    state.factory.asList().map { Triple(it.relativePath, it.sourceFiles.map(::key).sorted(), it.asByteArray()) }
                } finally { state.destroy() }
            }
            val headers = generate(ClassBuilderMode.KAPT3, files)
            check(bindingTrace.bindingContext === context && diagnostics() == before)
            val full = try { generate(ClassBuilderMode.FULL, producers) } catch (failure: Throwable) {
                facts["fullFailure"] = failure.javaClass.name + ": " + failure.message
                emptyList()
            }
            val outputs = (headers + full).associateBy { it.first }.values.toList()
            check(bindingTrace.bindingContext === context && diagnostics() == before)
            check(outputs.isNotEmpty() && outputs.map { it.first }.toSet().size == outputs.size)
            facts["outputs"] = outputs.map { (relative, sources, bytes) ->
                check(sources.isNotEmpty() && sources.all { it in expected })
                val target = File(out, relative).canonicalFile
                check(key(target).startsWith(key(out) + "/"))
                target.parentFile.mkdirs(); target.writeBytes(bytes)
                mapOf("relativePath" to relative, "sources" to sources, "sha256" to sha(bytes))
            }
            files.forEach { check(sha(File(it.virtualFile.path).readBytes()) == expected.getValue(key(File(it.virtualFile.path)))) }
            facts["bindingDiagnosticsPreserved"] = true
            facts["stageCompleted"] = true
        } catch (failure: Throwable) {
            facts["failure"] = failure.javaClass.name + ": " + failure.message
        }
        val temporary = File(receipt.parentFile, receipt.name + ".tmp")
        temporary.writeText(json(facts) + "\n", Charsets.UTF_8)
        check(temporary.renameTo(receipt))
        return AnalysisResult.success(context, module, shouldGenerateCode = false)
    }
}
