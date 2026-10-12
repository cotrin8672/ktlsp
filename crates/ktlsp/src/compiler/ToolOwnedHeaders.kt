@file:Suppress("DEPRECATION")
@file:OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class,
    org.jetbrains.kotlin.backend.common.extensions.FirIncompatiblePluginAPI::class)

package ktlsp.headers

import java.io.File
import java.security.MessageDigest
import org.jetbrains.kotlin.com.intellij.mock.MockProject
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.com.intellij.psi.PsiErrorElement
import org.jetbrains.kotlin.com.intellij.psi.PsiRecursiveElementWalkingVisitor
import org.jetbrains.kotlin.analyzer.AnalysisResult
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.jvm.JvmIrCodegenFactory
import org.jetbrains.kotlin.cli.common.CLIConfigurationKeys
import org.jetbrains.kotlin.cli.jvm.config.JavaSourceRoot
import org.jetbrains.kotlin.codegen.ClassBuilderMode
import org.jetbrains.kotlin.codegen.KotlinCodegenFacade
import org.jetbrains.kotlin.codegen.OriginCollectingClassBuilderFactory
import org.jetbrains.kotlin.codegen.extensions.ClassBuilderInterceptorExtension
import org.jetbrains.kotlin.codegen.extensions.ClassFileFactoryFinalizerExtension
import org.jetbrains.kotlin.codegen.state.GenerationState
import org.jetbrains.kotlin.compiler.plugin.*
import org.jetbrains.kotlin.config.*
import org.jetbrains.kotlin.descriptors.*
import org.jetbrains.kotlin.diagnostics.Severity
import org.jetbrains.kotlin.diagnostics.DiagnosticReporterFactory
import org.jetbrains.kotlin.load.java.components.TraceBasedErrorReporter
import org.jetbrains.kotlin.resolve.jvm.JvmBindingContextSlices
import org.jetbrains.kotlin.psi.*
import org.jetbrains.kotlin.resolve.BindingContext
import org.jetbrains.kotlin.resolve.BindingTrace
import org.jetbrains.kotlin.resolve.jvm.extensions.AnalysisHandlerExtension
import org.jetbrains.kotlin.types.KotlinType
import org.jetbrains.kotlin.types.error.ErrorUtils

private val OUTPUT = CompilerConfigurationKey.create<String>("ktlsp private API output")
private val RECEIPT = CompilerConfigurationKey.create<String>("ktlsp private API receipt")
private val INVENTORY = CompilerConfigurationKey.create<String>("ktlsp exact source inventory")
private val PLUGINS = CompilerConfigurationKey.create<String>("ktlsp pinned plugin/runtime inventory")
private val SELECTED_CP = CompilerConfigurationKey.create<String>("ktlsp selected ordered classpath")
private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
private fun key(file: File) = file.canonicalPath.replace('\\', '/').lowercase()
private fun json(v: Any?): String = when (v) {
    null -> "null"
    is Boolean, is Number -> v.toString()
    is Map<*, *> -> v.entries.joinToString(",", "{", "}") { json(it.key.toString()) + ":" + json(it.value) }
    is Iterable<*> -> v.joinToString(",", "[", "]") { json(it) }
    else -> "\"" + v.toString().flatMap { c ->
        when (c) {
            '"' -> "\\\"".toList()
            '\\' -> "\\\\".toList()
            '\n' -> "\\n".toList()
            '\r' -> "\\r".toList()
            '\t' -> "\\t".toList()
            else -> if (c.code < 32) "\\u%04x".format(c.code).toList() else listOf(c)
        }
    }.joinToString("") + "\""
}

class HeaderOptions : CommandLineProcessor {
    override val pluginId = "ktlsp.current-api-headers"
    override val pluginOptions = listOf("output", "receipt", "inventory", "plugins", "classpath").map {
        CliOption(it, "absolute-path", "Tool-owned private probe input/output", required = true, allowMultipleOccurrences = false)
    }
    override fun processOption(option: AbstractCliOption, value: String, configuration: CompilerConfiguration) {
        configuration.put(when (option.optionName) {
            "output" -> OUTPUT
            "receipt" -> RECEIPT
            "inventory" -> INVENTORY
            "plugins" -> PLUGINS
            "classpath" -> SELECTED_CP
            else -> error("Unknown tool-owned header option")
        }, value)
    }
}

class HeaderRegistrar : ComponentRegistrar {
    override fun registerProjectComponents(project: MockProject, configuration: CompilerConfiguration) {
        AnalysisHandlerExtension.registerExtension(project, HeaderExtension(configuration))
    }
}

private class HeaderExtension(private val originalConfiguration: CompilerConfiguration) : AnalysisHandlerExtension {
    private var calls = 0

    // Default doAnalysis returns null: the compiler performs normal full body analysis first.
    override fun analysisCompleted(project: Project, module: ModuleDescriptor, bindingTrace: BindingTrace,
        files: Collection<KtFile>): AnalysisResult? {
        val context = bindingTrace.bindingContext
        val receipt = File(originalConfiguration.getNotNull(RECEIPT))
        val out = File(originalConfiguration.getNotNull(OUTPUT)).canonicalFile
        val facts = linkedMapOf<String, Any?>("schema" to 1, "component" to "tool-owned-compiler-core-header-probe",
            "gradleTaskReplay" to false, "producerClosure" to false, "abiParityProven" to false,
            "stageCompleted" to false, "headersUsable" to false, "callbackCount" to ++calls,
            "compilerVersion" to KotlinCompilerVersion.getVersion())
        var domainProof: PackageDomainProof? = null
        fun diagnosticKeys() = context.diagnostics.all().map { d ->
            listOf(d.factory.name, d.severity.name, key(File(d.psiFile.virtualFile.path)),
                d.textRanges.joinToString(";") { it.startOffset.toString() + ":" + it.endOffset }).joinToString("|")
        }.sorted()
        val before = diagnosticKeys()
        facts["originalDiagnosticKeys"] = before
        try {
            check(calls == 1) { "Repeated analysis callback is unsupported" }
            check(!receipt.exists()) { "Private receipt must be absent" }
            check(facts["compilerVersion"] == "1.8.21") { "Only actual Kotlin 1.8.21 is source-verified" }
            check(!originalConfiguration.getBoolean(CommonConfigurationKeys.USE_FIR)) { "FIR/K2 is unsupported" }
            check(originalConfiguration.getBoolean(JVMConfigurationKeys.IR)) { "Preserve selected IR; no legacy fallback" }
            val incompleteHierarchies = context.getKeys(TraceBasedErrorReporter.INCOMPLETE_HIERARCHY)
            val badJavaSignatures = context.getKeys(JvmBindingContextSlices.LOAD_FROM_JAVA_SIGNATURE_ERRORS)
            facts["incompleteHierarchyCount"] = incompleteHierarchies.size
            facts["badJavaSignatureCount"] = badJavaSignatures.size
            check(incompleteHierarchies.isEmpty() && badJavaSignatures.isEmpty()) { "Additional compiler API error slices invalidate all headers" }
            val handlers = AnalysisHandlerExtension.getInstances(project)
            val ir = IrGenerationExtension.getInstances(project)
            val interceptors = ClassBuilderInterceptorExtension.getInstances(project)
            val finalizers = ClassFileFactoryFinalizerExtension.getInstances(project)
            facts["analysisHandlers"] = handlers.map { it.javaClass.name }
            facts["irExtensions"] = ir.map { mapOf("class" to it.javaClass.name,
                "stubOptIn" to it.shouldAlsoBeAppliedInKaptStubGenerationMode) }
            facts["classBuilderInterceptors"] = interceptors.map { it.javaClass.name }
            facts["classFileFinalizers"] = finalizers.map { it.javaClass.name }
            check(handlers.size == 1 && handlers.single() === this) { "Other analysis handlers have unproved fidelity" }
            // Stub opt-in is necessary for invocation, not sufficient evidence of ABI fidelity.
            check(ir.isEmpty() && interceptors.isEmpty() && finalizers.isEmpty()) { "Output-changing plugin hooks need an independent ABI control" }
            check(out.isDirectory && out.listFiles()!!.isEmpty()) { "Fresh private header output must be empty" }
            val expected = File(originalConfiguration.getNotNull(INVENTORY)).readLines().map { line ->
                val parts = line.split('\t')
                check(parts.size == 3) { "Malformed exact source inventory" }
                Triple(parts[0], parts[1], File(parts[2]).canonicalFile)
            }
            check(expected.map { key(it.third) }.toSet().size == expected.size) { "Duplicate source inventory entry" }
            expected.forEach { check(sha(it.third.readBytes()) == it.second) { "Source changed: " + it.third } }
            val kt = expected.filter { it.first == "kotlin" }.associateBy { key(it.third) }
            val java = expected.filter { it.first == "java" }.associateBy { key(it.third) }
            check(expected.size == kt.size + java.size) { "Unsupported source kind" }
            check(files.map { key(File(it.virtualFile.path)) }.toSet() == kt.keys && files.size == kt.size) { "KtFile inventory differs from all selected Kotlin sources" }
            files.forEach { check(sha(it.text.toByteArray(Charsets.UTF_8)) == kt.getValue(key(File(it.virtualFile.path))).second) { "Compiler PSI text differs from exact overlay" } }
            val javaRoots = originalConfiguration.getList(CLIConfigurationKeys.CONTENT_ROOTS).filterIsInstance<JavaSourceRoot>().map { it.file }
            facts["javaRoots"] = javaRoots.map { it.canonicalPath }
            check(javaRoots.all { root -> root.isFile && root.extension == "java" && key(root) in java }) { "Java roots must be exact selected files, never broad source directories" }
            check(javaRoots.map(::key).toSet() == java.keys) { "Java root inventory differs from all selected Java sources" }
            facts["sourceInventory"] = expected.map { mapOf("kind" to it.first, "sha256" to it.second, "path" to it.third.canonicalPath) }

            val proof = PackageDomainProof(originalConfiguration, project as MockProject, context, files, File(originalConfiguration.getNotNull(PLUGINS)), File(originalConfiguration.getNotNull(SELECTED_CP)))
            domainProof = proof
            facts["packageDomainProof"] = proof.facts

            val unknown = linkedMapOf<String, MutableList<String>>()
            val errors = context.diagnostics.all().filter { it.severity == Severity.ERROR }
            fun mark(file: KtFile, why: String) { unknown.getOrPut(key(File(file.virtualFile.path))) { mutableListOf() }.add(why) }
            for (file in files) {
                val knownBodies = mutableListOf<org.jetbrains.kotlin.com.intellij.openapi.util.TextRange>()
                file.accept(object : PsiRecursiveElementWalkingVisitor() {
                    override fun visitErrorElement(element: PsiErrorElement) { mark(file, "PSI syntax error at " + element.textRange.startOffset); super.visitErrorElement(element) }
                })
                file.accept(object : KtTreeVisitorVoid() {
                    override fun visitNamedDeclaration(declaration: KtNamedDeclaration) {
                        val descriptor = context[BindingContext.DECLARATION_TO_DESCRIPTOR, declaration]
                        val types = mutableListOf<KotlinType>()
                        when (descriptor) {
                            is CallableDescriptor -> {
                                descriptor.returnType?.let(types::add)
                                types.addAll(descriptor.valueParameters.map { it.type })
                                types.addAll(descriptor.typeParameters.flatMap { it.upperBounds })
                                descriptor.extensionReceiverParameter?.type?.let(types::add)
                                descriptor.dispatchReceiverParameter?.type?.let(types::add)
                            }
                            is ClassDescriptor -> {
                                types.addAll(descriptor.typeConstructor.supertypes)
                                types.addAll(descriptor.declaredTypeParameters.flatMap { it.upperBounds })
                            }
                            is TypeAliasDescriptor -> { types.add(descriptor.expandedType); types.add(descriptor.underlyingType) }
                        }
                        val knownSignature = descriptor != null && !ErrorUtils.isError(descriptor) &&
                            types.none { ErrorUtils.containsErrorType(it) || ErrorUtils.containsUninferredTypeVariable(it) }
                        if (!knownSignature) {
                            mark(file, "Unresolved signature " + declaration.name + "@" + declaration.textRange.startOffset)
                        }
                        val inferred = (declaration is KtNamedFunction && declaration.typeReference == null && !declaration.hasBlockBody()) ||
                            (declaration is KtProperty && declaration.typeReference == null)
                        val bodyStart = when (declaration) {
                            is KtNamedFunction -> declaration.bodyExpression?.textRange?.startOffset
                            is KtProperty -> declaration.initializer?.textRange?.startOffset
                            is KtClassOrObject -> declaration.body?.textRange?.startOffset
                            else -> null
                        }
                        val end = if (inferred) declaration.textRange.endOffset else bodyStart ?: declaration.textRange.endOffset
                        if (errors.any { it.psiFile === file && it.textRanges.any { r -> r.startOffset >= declaration.textRange.startOffset && r.startOffset < end } }) {
                            mark(file, "Signature/inference diagnostic " + declaration.name + "@" + declaration.textRange.startOffset)
                        }
                        if (knownSignature && declaration is KtNamedFunction &&
                            (declaration.typeReference != null || declaration.hasBlockBody())) {
                            declaration.bodyExpression?.textRange?.let(knownBodies::add)
                        }
                        super.visitNamedDeclaration(declaration)
                    }
                })
                // File annotations, imports and other unmatched errors can affect Java API.
                if (errors.any { d -> d.psiFile === file && d.textRanges.any { r ->
                    knownBodies.none { it.contains(r) }
                } }) mark(file, "Frontend error outside a proven explicit function body")
            }
            facts["unknownSources"] = unknown
            facts["allApiTypesKnown"] = unknown.isEmpty()
            val configuration = originalConfiguration.copy().apply { put(JVMConfigurationKeys.DO_NOT_CLEAR_BINDING_CONTEXT, true) }
            check(configuration.getBoolean(JVMConfigurationKeys.DO_NOT_CLEAR_BINDING_CONTEXT))
            val factory = OriginCollectingClassBuilderFactory(ClassBuilderMode.KAPT3)
            val backendReporter = DiagnosticReporterFactory.createReporter()
            val state = GenerationState.Builder(project, factory, module, context, configuration)
                .isIrBackend(true).ignoreErrors(true).diagnosticReporter(backendReporter).build()
            val outputs = try {
                KotlinCodegenFacade.compileCorrectFiles(files, state, JvmIrCodegenFactory(configuration, configuration.get(CLIConfigurationKeys.PHASE_CONFIG)))
                val extra = state.collectedExtraJvmDiagnostics.all()
                facts["extraJvmDiagnostics"] = extra.map { it.factory.name + ":" + it.severity.name }
                facts["irDiagnosticErrors"] = backendReporter.hasErrors
                check(extra.none { it.severity == Severity.ERROR } && !backendReporter.hasErrors) {
                    "JVM signature/backend errors invalidate all header observations"
                }
                state.factory.asList().map { output ->
                    Triple(output.relativePath, output.sourceFiles.map(::key).sorted(), output.asByteArray())
                }
            } finally { state.destroy() }
            check(bindingTrace.bindingContext === context && diagnosticKeys() == before) { "Original binding diagnostics changed during header emission" }
            check(outputs.map { it.first }.toSet().size == outputs.size) { "Duplicate output path" }
            val represented = outputs.filter { it.first.endsWith(".class") }.flatMap { it.second }.toSet()
            check(represented == kt.keys) { "Class headers do not account for every selected Kotlin source" }
            check(outputs.isNotEmpty()) { "No headers emitted" }
            val records = outputs.map { (relative, sources, bytes) ->
                check(sources.isNotEmpty() && sources.all { it in kt }) { "Unaccounted output source" }
                if (relative.endsWith(".class")) proof.inspectClass(relative, sources, bytes)
                val destination = File(out, relative).canonicalFile
                check(key(destination).startsWith(key(out) + "/")) { "Output escapes private root" }
                if (relative.endsWith(".class")) check(bytes.size >= 8 && bytes.take(4).map { it.toInt() and 255 } == listOf(202, 254, 186, 190)) { "Malformed class header" }
                destination.parentFile.mkdirs()
                destination.writeBytes(bytes)
                check(sha(destination.readBytes()) == sha(bytes)) { "Header write verification failed" }
                mapOf("relativePath" to relative, "sha256" to sha(bytes), "bytes" to bytes.size,
                    "sources" to sources, "apiStatus" to if (sources.any { it in unknown }) "Unknown" else "SignatureChecked")
            }
            val actualOutputs = out.walkTopDown().filter { it.isFile }.map { it.relativeTo(out).invariantSeparatorsPath }.toSet()
            check(actualOutputs == outputs.map { it.first.replace('\\', '/') }.toSet()) { "Private output inventory mismatch" }
            expected.forEach { check(sha(it.third.readBytes()) == it.second) { "Source changed while generating headers" } }
            proof.finish()
            facts["outputs"] = records
            facts["factoryDone"] = true
            facts["bindingIdentityPreserved"] = true
            facts["bindingDiagnosticsPreserved"] = true
            facts["allSelectedSourcesIncluded"] = true
            facts["stageCompleted"] = true
            facts["headersUsable"] = unknown.isEmpty()
            facts["partialSignatureObservationsOnly"] = unknown.isNotEmpty()
        } catch (e: Exception) {
            domainProof?.abort()
            facts["failure"] = e.javaClass.name + ": " + e.message
            facts["stageCompleted"] = false
            facts["headersUsable"] = false
        }
        receipt.parentFile.mkdirs()
        val temporary = File(receipt.parentFile, receipt.name + ".tmp")
        temporary.writeText(json(facts) + "\n", Charsets.UTF_8)
        check(temporary.renameTo(receipt)) { "Failed to publish final header receipt" }
        // Keep the original full-analysis diagnostics even when only API headers are emitted.
        return AnalysisResult.success(context, module, shouldGenerateCode = false)
    }
}
