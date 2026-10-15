import java.io.File
import java.nio.charset.Charset
import java.util.jar.JarEntry
import java.util.jar.JarOutputStream
import javax.tools.ToolProvider
import org.gradle.api.tasks.compile.JavaCompile

plugins { java }

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}
tasks.withType<JavaCompile>().configureEach { options.encoding = "UTF-8" }
repositories { maven { url = uri("project-repo") } }
dependencies { implementation("org.ktlsp.control:source-policy:1.0") }

val fixtureRoot = layout.buildDirectory.dir("policy-fixture")
val libraryGavPath = "org/ktlsp/control/source-policy/1.0"
val parentGavPath = "org/ktlsp/control/encoding-parent/1.0"
fun rootPom(encoding: String?, parent: String = "") =
    "<project xmlns=\"http://maven.apache.org/POM/4.0.0\"><modelVersion>4.0.0</modelVersion>" +
    parent + "<groupId>org.ktlsp.control</groupId><artifactId>source-policy</artifactId><version>1.0</version>" +
    (encoding?.let { "<properties><project.build.sourceEncoding>$it</project.build.sourceEncoding></properties>" } ?: "") + "</project>"
val parentReference = "<parent><groupId>org.ktlsp.control</groupId><artifactId>encoding-parent</artifactId><version>1.0</version></parent>"

tasks.register("prepareRepositories") {
    doLast {
        val compiler = ToolProvider.getSystemJavaCompiler() ?: error("JDK 17+ is required")
        fun jar(path: File, entry: String, bytes: ByteArray) {
            path.parentFile.mkdirs()
            JarOutputStream(path.outputStream()).use { archive ->
                archive.putNextEntry(JarEntry(entry))
                archive.write(bytes)
                archive.closeEntry()
            }
        }
        for (repository in listOf("settings", "project")) {
            val encoding = if (repository == "settings") "UTF-8" else "ISO-8859-1"
            val header = if (repository == "settings") "// settings UTF-8\n" else "// project inherited ISO\n\n\n"
            val text = header + "package org.ktlsp.control; public class Policy { public int café() { return 1; } public int known() { return 2; } }\n"
            val input = fixtureRoot.get().file("$repository/Policy.java").asFile
            input.parentFile.mkdirs()
            input.writeBytes(text.toByteArray(Charset.forName(encoding)))
            val classes = fixtureRoot.get().dir("$repository/classes").asFile
            classes.mkdirs()
            check(compiler.run(null, null, null, "--release", "17", "-encoding", encoding,
                "-d", classes.absolutePath, input.absolutePath) == 0)
            val base = file("$repository-repo/$libraryGavPath")
            jar(File(base, "source-policy-1.0.jar"), "org/ktlsp/control/Policy.class",
                File(classes, "org/ktlsp/control/Policy.class").readBytes())
            jar(File(base, "source-policy-1.0-sources.jar"), "org/ktlsp/control/Policy.java", input.readBytes())
            File(base, "source-policy-1.0.pom").writeText(
                if (repository == "settings") rootPom("UTF-8") else rootPom(null, parentReference))
            val oracle = fixtureRoot.get().file("oracle/$repository/Policy.java").asFile
            oracle.parentFile.mkdirs()
            oracle.writeText(text, Charsets.UTF_8)
        }
        val parent = file("project-repo/$parentGavPath/encoding-parent-1.0.pom")
        parent.parentFile.mkdirs()
        parent.writeText("<project xmlns=\"http://maven.apache.org/POM/4.0.0\"><modelVersion>4.0.0</modelVersion><groupId>org.ktlsp.control</groupId><artifactId>encoding-parent</artifactId><version>1.0</version><packaging>pom</packaging><properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties></project>")
    }
}

// Change the selected artifact after Gradle has read the valid dependency graph. This exercises
// the production discovery guards, rather than Gradle's own initial POM parser.
val metadataGuard = providers.gradleProperty("metadataGuard")
    .orElse(providers.gradleProperty("metadataGuard.${project.name}")).orNull
if (metadataGuard != null) {
    gradle.projectsEvaluated {
        tasks.matching { it.name == "ktlspDumpSources" }.configureEach {
            doFirst {
                configurations.getByName("compileClasspath").resolve()
                configurations.getByName("testCompileClasspath").resolve()
                val parent = when (metadataGuard) {
                    "namespace" -> "<x:parent xmlns:x=\"urn:foreign\"><x:groupId>org.ktlsp.control</x:groupId><x:artifactId>trap</x:artifactId><x:version>1.0</x:version></x:parent>"
                    "nested" -> "<parent><groupId><value>org.ktlsp.control</value></groupId><artifactId>trap</artifactId><version>1.0</version></parent>"
                    "duplicate" -> parentReference + parentReference
                    "interpolated" -> parentReference.replace("1.0", "\${revision}")
                    "dotdot" -> parentReference.replace("1.0", "..")
                    else -> ""
                }
                var payload = rootPom("UTF-8", parent)
                if (metadataGuard == "doctype") {
                    val sentinel = fixtureRoot.get().file("external-entity.txt").asFile
                    sentinel.parentFile.mkdirs()
                    sentinel.writeText("org.ktlsp.forbidden")
                    payload = "<!DOCTYPE project [<!ENTITY external SYSTEM \"${sentinel.toURI()}\">]>" +
                        rootPom("UTF-8", "<parent><groupId>&external;</groupId><artifactId>trap</artifactId><version>1.0</version></parent>")
                } else if (metadataGuard == "oversize") {
                    payload = payload.replace("</project>", "<!--" + "x".repeat((1 shl 20) + 1) + "--></project>")
                }
                check(metadataGuard in listOf("namespace", "nested", "duplicate", "interpolated", "dotdot", "doctype", "oversize"))
                val repository = if (providers.gradleProperty("repositoryPolicy").orNull == "project") "project" else "settings"
                file("$repository-repo/$libraryGavPath/source-policy-1.0.pom").writeText(payload, Charsets.UTF_8)
            }
        }
    }
}
