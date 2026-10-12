import org.gradle.api.tasks.compile.JavaCompile
import org.gradle.jvm.toolchain.JavaLanguageVersion
import org.gradle.jvm.toolchain.JavaToolchainService

plugins { java }
java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

val apDisabled = sourceSets.create("apDisabled")
val forked = sourceSets.create("forked")
val forkHome = sourceSets.create("forkHome")
val forkExecutable = sourceSets.create("forkExecutable")
val forkJvmArgs = sourceSets.create("forkJvmArgs")
val custom = sourceSets.create("custom")
val controlCompiler = extensions.getByType(JavaToolchainService::class.java).compilerFor {
    languageVersion.set(JavaLanguageVersion.of(17))
}
tasks.named<JavaCompile>(apDisabled.compileJavaTaskName) { options.compilerArgs.add("-proc:none") }
tasks.named<JavaCompile>(forked.compileJavaTaskName) { options.isFork = true }
tasks.named<JavaCompile>(forkHome.compileJavaTaskName) {
    options.forkOptions.javaHome = controlCompiler.get().metadata.installationPath.asFile
}
tasks.named<JavaCompile>(forkExecutable.compileJavaTaskName) {
    options.forkOptions.executable = controlCompiler.get().executablePath.asFile.absolutePath
}
tasks.named<JavaCompile>(forkJvmArgs.compileJavaTaskName) { options.forkOptions.jvmArgs = listOf("-Xmx96m") }

abstract class CustomJavaCompile : JavaCompile()
tasks.replace(custom.compileJavaTaskName, CustomJavaCompile::class.java).apply {
    setSource(custom.java)
    classpath = custom.compileClasspath
    destinationDirectory.set(project.layout.buildDirectory.dir("classes/java/custom"))
    sourceCompatibility = "17"
    targetCompatibility = "17"
}
