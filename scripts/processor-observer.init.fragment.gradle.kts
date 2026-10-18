// Fixed selected KSP -> stubs -> KAPT observation in ONE owned private invocation.
// Configuration/JSON is descriptive; only the native host constructs completion capabilities.
val currentProducerDirectory = System.getProperty("ktlsp.processor.proof.dir")
val currentProducerNonce = System.getProperty("ktlsp.processor.proof.nonce")
val currentProducerOwner = System.getProperty("ktlsp.processor.owner")
if (currentProducerDirectory != null && currentProducerNonce != null && currentProducerOwner != null) {
    gradle.taskGraph.whenReady(object : Action<org.gradle.api.execution.TaskExecutionGraph> {
        override fun execute(graph: org.gradle.api.execution.TaskExecutionGraph) {
            check(gradle.gradleVersion == "7.4.2") { "Unproved processor Gradle profile" }
            check(currentProducerOwner.endsWith("compileKotlin"))
            val prefix = currentProducerOwner.removeSuffix("compileKotlin")
            val wanted = listOf(prefix + "kspKotlin", prefix + "kaptGenerateStubsKotlin", prefix + "kaptKotlin")
            check(wanted.all { name -> graph.allTasks.count { it.path == name } == 1 })
            check(graph.allTasks.none { it.path == currentProducerOwner || it.path == prefix + "compileJava" }) {
                "Consumer compilation is not a producer phase"
            }
            val module = graph.allTasks.single { it.path == wanted.first() }.project
            val observer = KtlspSelectedTaskObserver
            observer.directory = java.io.File(currentProducerDirectory)
            observer.nonce = currentProducerNonce
            check(observer.directory.isDirectory)
            fun onlyIf(task: Task) {
                val fact = KtlspCompilerConfiguration.producerOnlyIf(task) ?: error("Task predicate contract unavailable")
                val rows = fact["predicates"] as? List<*> ?: error("Task predicate contract unavailable")
                check(rows.size == 1 && (rows.single() as? Map<*, *>)?.get("stockEnabledPredicate") == true) {
                    "Custom bootstrap task predicate"
                }
            }
            val graphContracts = graph.allTasks.map { task ->
                task.usesService(compilerRuntimes)
                check(task.project.rootProject == gradle.rootProject) { "Included producer build is unproved" }
                val privateRoot = gradle.rootProject.rootDir.toPath().toAbsolutePath().normalize()
                check(task.outputs.files.files.all { output ->
                    val path = output.toPath().toAbsolutePath().normalize()
                    path != privateRoot && path.startsWith(privateRoot)
                }) { "Declared producer output escapes the private current generation" }
                val type = task.javaClass.name
                if (task.path !in wanted) {
                    onlyIf(task)
                    val actions = KtlspCompilerConfiguration.producerActions(task) ?: error("Bootstrap actions unavailable")
                    val rows = actions["actions"] as? List<*> ?: error("Bootstrap actions unavailable")
                    when (type) {
                        "org.gradle.api.DefaultTask_Decorated" -> check(task.name == "classes" && rows.isEmpty())
                        "org.gradle.api.tasks.bundling.Jar_Decorated",
                        "org.gradle.language.jvm.tasks.ProcessResources_Decorated" -> {
                            check(rows.size == 1)
                            val action = rows.single() as? Map<*, *> ?: error("Bootstrap action unavailable")
                            check(action["class"] == "org.gradle.api.internal.project.taskfactory.StandardTaskAction" &&
                                action["declaringClass"] == "org.gradle.api.tasks.AbstractCopyTask" &&
                                action["method"] == "copy" && action["parameterTypes"] == emptyList<String>()) {
                                "Custom resource/archive action"
                            }
                        }
                        "org.gradle.api.tasks.compile.JavaCompile_Decorated",
                        "org.jetbrains.kotlin.gradle.tasks.KotlinCompile_Decorated" -> {
                            val compiler = observer.configuration(task, compilerRuntimes)
                            check(compiler["complete"] == true && compiler["problem"] == null)
                            val options = compiler["options"] as? Map<*, *> ?: error("Bootstrap compiler options unavailable")
                            val actionRows = (options["taskActionInventory"] as? Map<*, *>)?.get("actions") as? List<*>
                                ?: error("Bootstrap compiler action unavailable")
                            check(actionRows.size == 1)
                            val action = actionRows.single() as? Map<*, *> ?: error("Bootstrap compiler action unavailable")
                            check(action["class"] == "org.gradle.api.internal.project.taskfactory.IncrementalInputsTaskAction")
                            check(action["parameterTypes"] == listOf("org.gradle.work.InputChanges"))
                            if (task is JavaCompile) {
                                check(action["declaringClass"] == "org.gradle.api.tasks.compile.JavaCompile" && action["method"] == "compile")
                                check(options["fork"] == false && options["forkJavaHome"] == null &&
                                    options["forkExecutable"] == null && options["forkJvmArgs"] == emptyList<String>() &&
                                    compiler["annotation_processor_path"] == emptyList<String>() &&
                                    options["javaCompilerRecipe"] != null && options["javaCompilerRecipeProblem"] == null)
                            } else {
                                check(action["declaringClass"] == "org.jetbrains.kotlin.gradle.tasks.AbstractKotlinCompile" && action["method"] == "execute")
                                val runtime = compiler["kotlin_compiler"] as? Map<*, *> ?: error("Bootstrap Kotlin runtime unavailable")
                                check(runtime["compiler_version"] == "1.8.21" && runtime["gradle_plugin_version"] == "1.8.21" &&
                                    runtime["execution_strategy"] == "IN_PROCESS" && runtime["run_via_build_tools_api"] == false)
                            }
                        }
                        else -> error("Unsupported bootstrap producer task: $type")
                    }
                }
                linkedMapOf("task" to task.path, "taskType" to type, "projectDir" to observer.path(task.project.projectDir),
                    "declaredOutputPaths" to task.outputs.files.files.map { observer.path(it) },
                    "actions" to KtlspCompilerConfiguration.producerActions(task),
                    "predicates" to KtlspCompilerConfiguration.producerOnlyIf(task))
            }
            observer.event("processor-graph", linkedMapOf(
                "root" to observer.path(gradle.rootProject.rootDir), "tasks" to graph.allTasks.map { it.path },
                "contracts" to graphContracts, "gradleVersion" to gradle.gradleVersion,
                "javaHome" to System.getProperty("java.home"), "gradleUserHome" to observer.path(gradle.gradleUserHomeDir),
                "requestedTasks" to gradle.startParameter.taskNames,
                "moduleSources" to observer.inventory(java.io.File(module.projectDir, "src/main")),
                "producerCompletion" to false, "apiAuthority" to false))
            var completedCompilerConfigurations: List<Map<*, *>>? = null
            @Suppress("DEPRECATION")
            graph.addTaskExecutionListener(object : org.gradle.api.execution.TaskExecutionListener {
                override fun beforeExecute(task: Task) {
                    if (task.path !in wanted) {
                        observer.event("processor-bootstrap-start", linkedMapOf("task" to task.path,
                            "inputs" to observer.inventory(task.inputs.files.files.sortedBy { observer.path(it) })))
                        return
                    }
                    observer.event("processor-start", linkedMapOf("task" to task.path,
                        "configuration" to observer.configuration(task, compilerRuntimes),
                        "inputs" to observer.inventory(task.inputs.files.files.sortedBy { observer.path(it) }),
                        "outputsBefore" to observer.inventory(task.outputs.files.files.toList())))
                }
                override fun afterExecute(task: Task, state: org.gradle.api.tasks.TaskState) {
                    observer.event("processor-task-end", linkedMapOf("task" to task.path,
                        "state" to observer.state(state), "outputs" to observer.inventory(task.outputs.files.files.toList())))
                    if (task.path == wanted.last() && state.failure == null) {
                        val configurations = listOf(currentProducerOwner, prefix + "compileJava")
                            .map { observer.configuration(module.tasks.getByPath(it), compilerRuntimes) }
                        check(configurations.all { it["complete"] == true && it["problem"] == null }) {
                            "Current completed producer compiler configuration unavailable"
                        }
                        completedCompilerConfigurations = configurations
                    }
                }
            })
            @Suppress("DEPRECATION")
            gradle.buildFinished(object : Action<org.gradle.BuildResult> {
                override fun execute(result: org.gradle.BuildResult) {
                    observer.event("processor-build-finished", linkedMapOf("failure" to result.failure?.toString(),
                        "moduleSources" to observer.inventory(java.io.File(module.projectDir, "src/main")),
                        "generatedSources" to observer.inventory(java.io.File(module.buildDir, "generated")),
                        "selectedCompilerConfigurations" to checkNotNull(completedCompilerConfigurations) {
                            "Current producer did not seal its compiler configuration"
                        }))
                }
            })
        }
    })
}
