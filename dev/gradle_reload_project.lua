-- Disposable fixture for the existing gradle-live editor harness.
local M = {}

local function write(path, text)
  vim.fn.mkdir(vim.fs.dirname(path), 'p')
  vim.fn.writefile(vim.split(text, '\n', { plain = true }), path)
end

local function seed_artifact(cache, root, group, artifact, version)
  local from = cache .. '/modules-2/files-2.1/' .. group .. '/' .. artifact .. '/' .. version
  if vim.fn.isdirectory(from) == 0 then return end
  local to = root .. '/home/.m2/repository/' .. group:gsub('%.', '/') .. '/' .. artifact .. '/' .. version
  vim.fn.mkdir(to, 'p')
  for hash, kind in vim.fs.dir(from) do
    if kind == 'directory' then
      for name, file_kind in vim.fs.dir(from .. '/' .. hash) do
        if file_kind == 'file' and (name:match('%.jar$') or name:match('%.pom$')) then
          assert(vim.uv.fs_copyfile(from .. '/' .. hash .. '/' .. name, to .. '/' .. name))
        end
      end
    end
  end
end

function M.write(root, repo)
  assert(vim.fn.isdirectory(root) == 0, 'reload fixture destination must be new')
  vim.fn.mkdir(root .. '/gradle/wrapper', 'p')
  for _, file in ipairs({ 'gradlew', 'gradlew.bat', 'gradle/wrapper/gradle-wrapper.jar' }) do
    assert(vim.uv.fs_copyfile(repo .. '/dev/gradle-wrapper-template/' .. file, root .. '/' .. file))
  end
  if vim.fn.has('win32') == 0 then vim.uv.fs_chmod(root .. '/gradlew', 493) end
  write(root .. '/gradle/wrapper/gradle-wrapper.properties', [[distributionBase=GRADLE_USER_HOME
distributionPath=wrapper/dists
distributionUrl=https\://services.gradle.org/distributions/gradle-9.6.1-bin.zip
networkTimeout=10000
validateDistributionUrl=true
zipStoreBase=GRADLE_USER_HOME
zipStorePath=wrapper/dists]])
  write(root .. '/settings.gradle', [[pluginManagement { repositories { gradlePluginPortal(); mavenCentral() } }
rootProject.name = 'ktlsp-live-reload'
include(':app')
project(':app').projectDir = file('modules/editing')]])
  write(root .. '/gradle.properties', [[org.gradle.parallel=false
org.gradle.workers.max=2
kotlin.compiler.execution.strategy=in-process]])
  write(root .. '/build.gradle', [=[plugins { id 'java' }
def published = []
[['V1', 'versioned', '1.0'], ['V2', 'versioned', '2.0'], ['Added', 'added', '1.0']].each { spec ->
    def suffix = spec[0]
    def artifact = spec[1]
    def version = spec[2]
    def sourceDir = layout.buildDirectory.dir("fixture-src/${suffix}")
    def targetDir = layout.projectDirectory.dir("home/.m2/repository/fixture/reload/${artifact}/${version}")
    def compilation = tasks.register("compileFixture${suffix}", JavaCompile) {
        source(sourceDir)
        classpath = files()
        destinationDirectory = layout.buildDirectory.dir("fixture-classes/${suffix}")
        options.release = 17
    }
    published.add(tasks.register("jarFixture${suffix}", Jar) {
        dependsOn(compilation)
        from(compilation.map { it.destinationDirectory })
        destinationDirectory = targetDir
        archiveFileName = "${artifact}-${version}.jar"
    })
    published.add(tasks.register("sourcesFixture${suffix}", Jar) {
        from(sourceDir)
        destinationDirectory = targetDir
        archiveFileName = "${artifact}-${version}-sources.jar"
    })
}
tasks.register('fakeJdkSources', Jar) {
    from(layout.buildDirectory.dir('fake-jdk-src'))
    destinationDirectory = layout.buildDirectory
    archiveFileName = 'fake-jdk-src.zip'
}
tasks.register('fixtureJars') { dependsOn(published, 'fakeJdkSources') }]=])
  write(root .. '/build/fake-jdk-src/java/lang/Object.java', 'package java.lang;\npublic class Object { }\n')
  for _, version in ipairs({ '1.0', '2.0' }) do
    local suffix = version == '1.0' and 'V1' or 'V2'
    local member = version == '1.0' and 'oldApi' or 'newApi'
    write(root .. '/build/fixture-src/' .. suffix .. '/vendor/Versioned.java',
      'package vendor;\n\npublic class Versioned {\n    public int ' .. member .. '() { return 7; }\n}\n')
  end
  write(root .. '/build/fixture-src/Added/vendor/Added.java', [[package vendor;

public class Added {
    public int addedApi() { return 9; }
}]])
  for _, spec in ipairs({ { 'versioned', '1.0' }, { 'versioned', '2.0' }, { 'added', '1.0' } }) do
    write(root .. '/home/.m2/repository/fixture/reload/' .. spec[1] .. '/' .. spec[2] .. '/' .. spec[1] .. '-' .. spec[2] .. '.pom',
      '<project><modelVersion>4.0.0</modelVersion><groupId>fixture.reload</groupId><artifactId>' .. spec[1]
      .. '</artifactId><version>' .. spec[2] .. '</version></project>')
  end
  write(root .. '/modules/editing/src/main/kotlin/app/KotlinHelper.kt', [[package app

class KotlinHelper {
    fun assist(): Int = 1
}]])
  -- Source artifacts are copied to the isolated server home; Gradle keeps its configured cache.
  local gradle_cache = vim.env.KTLSP_GRADLE_RELOAD_CACHE or (vim.env.GRADLE_USER_HOME and vim.env.GRADLE_USER_HOME .. '/caches')
  if gradle_cache then
    seed_artifact(gradle_cache, root, 'org.jetbrains.kotlin', 'kotlin-stdlib', '2.4.0')
    seed_artifact(gradle_cache, root, 'org.jetbrains', 'annotations', '13.0')
  end
  M.phase(root, 0)
end

function M.phase(root, phase)
  assert(phase >= 0 and phase <= 3)
  write(root .. '/phase.txt', tostring(phase))
  local version = phase < 2 and '1.0' or '2.0'
  local deps = phase == 3 and '' or "    implementation 'fixture.reload:versioned:" .. version .. "'\n"
  if phase == 1 or phase == 2 then deps = deps .. "    implementation 'fixture.reload:added:1.0'\n" end
  write(root .. '/modules/editing/build.gradle', [=[plugins { id 'org.jetbrains.kotlin.jvm' version '2.4.0'; id 'java' }
repositories {
    maven { url = rootProject.file('home/.m2/repository') }
    mavenCentral()
}
java { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
kotlin { compilerOptions { jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17 } }
sourceSets.main.java.srcDirs('build/generated/sources/fixture/java/main', 'build/ktlsp-custom-java')
def phaseFile = rootProject.layout.projectDirectory.file('phase.txt')
def standardDir = layout.buildDirectory.dir('generated/sources/fixture/java/main')
def customDir = layout.buildDirectory.dir('ktlsp-custom-java')
tasks.register('generateFixture') {
    inputs.file(phaseFile)
    outputs.dirs(standardDir, customDir)
    doLast {
        def active = phaseFile.asFile.text.trim() in ['1', '2']
        [[standardDir.get().asFile, 'StandardGenerated', 'generatedValue'],
         [customDir.get().asFile, 'CustomGenerated', 'customValue']].each { spec ->
            def source = new File(spec[0], "generated/${spec[1]}.java")
            if (active) {
                source.parentFile.mkdirs()
                source.text = "package generated;\n\npublic class ${spec[1]} {\n    public int ${spec[2]}() { return 3; }\n}\n"
            } else { source.delete() }
        }
    }
}
tasks.named('compileJava') { dependsOn(rootProject.tasks.named('fixtureJars'), 'generateFixture') }
tasks.named('compileKotlin') { dependsOn(rootProject.tasks.named('fixtureJars'), 'generateFixture') }
dependencies {
]=] .. deps .. '}\n')
  local imports, body = '', '        int value = new KotlinHelper().assist();\n'
  if phase < 3 then
    imports = imports .. 'import vendor.Versioned;\n'
    body = body .. '        value += new Versioned().' .. (phase < 2 and 'oldApi' or 'newApi') .. '();\n'
  end
  if phase == 1 or phase == 2 then
    imports = imports .. 'import vendor.Added;\nimport generated.StandardGenerated;\nimport generated.CustomGenerated;\n'
    body = body .. [[        value += new Added().addedApi();
        value += new StandardGenerated().generatedValue();
        value += new CustomGenerated().customValue();
]]
  end
  write(root .. '/modules/editing/src/main/java/app/Main.java', 'package app;\n\n' .. imports
    .. '\npublic class Main {\n    public int use() {\n' .. body .. '        return value;\n    }\n}\n')
  write(root .. '/modules/editing/src/main/kotlin/app/Use.kt', 'package app\n\n' .. imports:gsub(';', '')
    .. '\nfun useKotlin(): Int {\n' .. body:gsub('        int value = ', '    var value = '):gsub('        ', '    '):gsub('new ', ''):gsub(';', '')
    .. '    return value\n}\n')
end

return M
