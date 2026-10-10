-- Disposable compiler-plugin golden for the existing corpus and popup probes.
-- nvim -l dev/compiler_plugin_project.lua <new-external-directory>
local root = vim.fs.abspath(assert(arg[1], 'missing new fixture directory'))
local repo = vim.fs.dirname(vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', '')))
assert(vim.fn.isdirectory(root) == 0, 'compiler fixture destination must be new')
local function write(path, text)
  vim.fn.mkdir(vim.fs.dirname(path), 'p')
  vim.fn.writefile(vim.split(text:gsub('\n$', ''), '\n', { plain = true }), path)
end
for _, file in ipairs({ 'gradlew', 'gradlew.bat', 'gradle/wrapper/gradle-wrapper.jar',
  'gradle/wrapper/gradle-wrapper.properties' }) do
  vim.fn.mkdir(vim.fs.dirname(root .. '/' .. file), 'p')
  assert(vim.uv.fs_copyfile(repo .. '/dev/gradle-wrapper-template/' .. file, root .. '/' .. file))
end
if vim.fn.has('win32') == 0 then assert(vim.uv.fs_chmod(root .. '/gradlew', 493)) end
local files = {
  ['settings.gradle.kts'] = [[pluginManagement { repositories { gradlePluginPortal(); mavenCentral() } }
rootProject.name = "ktlsp-compiler-plugin-oracle"
]],
  ['gradle.properties'] = [[org.gradle.jvmargs=-Xmx1g -Dfile.encoding=UTF-8
org.gradle.workers.max=2
kotlin.compiler.execution.strategy=in-process
]],
  ['build.gradle.kts'] = [[plugins {
    kotlin("jvm") version "1.9.23"
    kotlin("plugin.serialization") version "1.9.23"
    kotlin("plugin.allopen") version "1.9.23"
    kotlin("plugin.noarg") version "1.9.23"
    kotlin("plugin.lombok") version "1.9.23"
}

repositories { mavenCentral() }
dependencies {
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-core:1.6.3")
    compileOnly("org.projectlombok:lombok:1.18.32")
    annotationProcessor("org.projectlombok:lombok:1.18.32")
}
kotlin { jvmToolchain(17) }
allOpen { annotation("pluginfixture.OpenForTesting") }
noArg { annotation("pluginfixture.GenerateNoArg") }

tasks.withType<JavaCompile>().configureEach { options.release.set(17) }
]],
  ['src/main/kotlin/pluginfixture/Model.kt'] = [[package pluginfixture

import kotlinx.serialization.Serializable

@Serializable
data class Model(val label: String)
]],
  ['src/main/kotlin/pluginfixture/OpenBase.kt'] = [[package pluginfixture

annotation class OpenForTesting
annotation class GenerateNoArg

@OpenForTesting
class OpenBase {
    fun value(): String = "base"
}

@GenerateNoArg
class NeedsLabel(val label: String)

class KotlinChild : OpenBase() {
    override fun value(): String = "kotlin"
}
]],
  ['src/main/kotlin/pluginfixture/KotlinUse.kt'] = [[package pluginfixture

fun serializerFromKotlin() = Model.serializer()
fun lombokFromKotlin() = Person("Kotlin").name
fun lombokMethodFromKotlin() = Person("method").getName()
]],
  ['src/main/java/pluginfixture/Person.java'] = [[package pluginfixture;

import lombok.AllArgsConstructor;
import lombok.Getter;

@Getter
@AllArgsConstructor
public final class Person {
    private String name;
}
]],
  ['src/main/java/pluginfixture/JavaUse.java'] = [[package pluginfixture;

public final class JavaUse {
    public static Object serializerFromJava() { return Model.Companion.serializer(); }
    public static String lombokFromJava() { return new Person("Java").getName(); }
    public static NeedsLabel noArgFromBinary() { return new NeedsLabel(); }
}
]],
  ['src/main/java/pluginfixture/JavaChild.java'] = [[package pluginfixture;

public final class JavaChild extends OpenBase {
    @Override public String value() { return "java"; }
}
]],
}
for path, text in pairs(files) do write(root .. '/' .. path, text) end
print('Compiler-plugin fixture: ' .. root)
vim.cmd('qa!')
