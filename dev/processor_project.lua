-- Real KAPT/KSP producers for the existing corpus probe; outputs are never handwritten.
-- nvim -l dev/processor_project.lua <new-external-directory>
local root = vim.fs.abspath(assert(arg[1], 'missing new fixture directory'))
local repo = vim.fs.dirname(vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', '')))
assert(vim.fn.isdirectory(root) == 0, 'processor fixture destination must be new')
local function write(path, text)
  vim.fn.mkdir(vim.fs.dirname(path), 'p')
  vim.fn.writefile(vim.split(text:gsub('\n$', ''), '\n', { plain = true }), path)
end
for _, file in ipairs({ 'gradlew', 'gradlew.bat', 'gradle/wrapper/gradle-wrapper.jar' }) do
  vim.fn.mkdir(vim.fs.dirname(root .. '/' .. file), 'p')
  assert(vim.uv.fs_copyfile(repo .. '/dev/gradle-wrapper-template/' .. file, root .. '/' .. file))
end
if vim.fn.has('win32') == 0 then assert(vim.uv.fs_chmod(root .. '/gradlew', 493)) end
local files = {
  ['gradle/wrapper/gradle-wrapper.properties'] = [[distributionBase=GRADLE_USER_HOME
distributionPath=wrapper/dists
distributionUrl=https\://services.gradle.org/distributions/gradle-7.4.2-bin.zip
zipStoreBase=GRADLE_USER_HOME
zipStorePath=wrapper/dists
]],
  ['settings.gradle.kts'] = [[pluginManagement { repositories { gradlePluginPortal(); mavenCentral() } }
dependencyResolutionManagement { repositories { mavenCentral() } }
rootProject.name = "processor-oracle"
include(":app", ":kapt-processor", ":ksp-processor")
]],
  ['build.gradle.kts'] = [[plugins {
    kotlin("jvm") version "1.8.21" apply false
    kotlin("kapt") version "1.8.21" apply false
    id("com.google.devtools.ksp") version "1.8.21-1.0.11" apply false
}
]],
  ['gradle.properties'] = [[generationLabel=v1
org.gradle.jvmargs=-Xmx1g -Dfile.encoding=UTF-8
org.gradle.workers.max=2
kotlin.compiler.execution.strategy=in-process
]],
  ['kapt-processor/build.gradle.kts'] = [[plugins { `java-library` }
java { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
]],
  ['kapt-processor/src/main/java/processor/GenerateKapt.java'] = [[package processor;
import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;
@Retention(RetentionPolicy.SOURCE)
@Target(ElementType.TYPE)
public @interface GenerateKapt {}
]],
  ['kapt-processor/src/main/java/processor/KaptGenerator.java'] = [[package processor;
import java.io.IOException;
import java.io.Writer;
import java.util.Set;
import javax.annotation.processing.AbstractProcessor;
import javax.annotation.processing.RoundEnvironment;
import javax.lang.model.SourceVersion;
import javax.lang.model.element.TypeElement;
public final class KaptGenerator extends AbstractProcessor {
    private boolean generated;
    @Override public Set<String> getSupportedAnnotationTypes() { return Set.of("processor.GenerateKapt"); }
    @Override public Set<String> getSupportedOptions() { return Set.of("generationLabel"); }
    @Override public SourceVersion getSupportedSourceVersion() { return SourceVersion.RELEASE_17; }
    @Override public boolean process(Set<? extends TypeElement> annotations, RoundEnvironment round) {
        if (generated || round.processingOver() || round.getElementsAnnotatedWith(GenerateKapt.class).isEmpty()) return false;
        generated = true;
        String label = processingEnv.getOptions().getOrDefault("generationLabel", "v1");
        if (!label.matches("v[12]")) throw new IllegalArgumentException("unexpected oracle label");
        String member = label.equals("v1") ? "versionOne" : "versionTwo";
        try (Writer writer = processingEnv.getFiler().createSourceFile("generated.KaptJava").openWriter()) {
            writer.write("package generated; public class KaptJava { public String value() { return \"kapt-" + label + "\"; } public String " + member + "() { return value(); } }");
        } catch (IOException failure) { throw new IllegalStateException(failure); }
        return true;
    }
}
]],
  ['kapt-processor/src/main/resources/META-INF/services/javax.annotation.processing.Processor'] = [[processor.KaptGenerator
]],
  ['ksp-processor/build.gradle.kts'] = [[plugins { kotlin("jvm") }
dependencies { implementation("com.google.devtools.ksp:symbol-processing-api:1.8.21-1.0.11") }
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach { kotlinOptions.jvmTarget = "17" }
]],
  ['ksp-processor/src/main/kotlin/processor/KspGenerator.kt'] = [[package processor
import com.google.devtools.ksp.processing.*
import com.google.devtools.ksp.symbol.KSAnnotated
class KspGeneratorProvider : SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment): SymbolProcessor = KspGenerator(environment)
}
class KspGenerator(private val environment: SymbolProcessorEnvironment) : SymbolProcessor {
    private var generated = false
    override fun process(resolver: Resolver): List<KSAnnotated> {
        if (generated || !resolver.getSymbolsWithAnnotation("fixture.GenerateKsp").iterator().hasNext()) return emptyList()
        generated = true
        val label = environment.options["generationLabel"] ?: "v1"
        require(label == "v1" || label == "v2")
        val member = if (label == "v1") "versionOne" else "versionTwo"
        val dependencies = Dependencies(false, *resolver.getAllFiles().toList().toTypedArray())
        environment.codeGenerator.createNewFile(dependencies, "generated", "KspKotlin", "kt").writer().use {
            it.write("package generated\nclass KspKotlin { fun value(): String = \"ksp-kotlin-$label\"; fun $member(): String = value() }\n")
        }
        environment.codeGenerator.createNewFile(dependencies, "generated", "KspJava", "java").writer().use {
            it.write("package generated; public class KspJava { public String value() { return \"ksp-java-$label\"; } public String $member() { return value(); } }")
        }
        return emptyList()
    }
}
]],
  ['ksp-processor/src/main/resources/META-INF/services/com.google.devtools.ksp.processing.SymbolProcessorProvider'] = [[processor.KspGeneratorProvider
]],
  ['app/build.gradle.kts'] = [[plugins { kotlin("jvm"); kotlin("kapt"); id("com.google.devtools.ksp") }
dependencies {
    implementation(project(":kapt-processor"))
    if (!providers.gradleProperty("processingDisabled").isPresent) {
        kapt(project(":kapt-processor"))
        ksp(project(":ksp-processor"))
    }
}
val generationLabel = providers.gradleProperty("generationLabel").getOrElse("v1")
kapt { arguments { arg("generationLabel", generationLabel) } }
ksp { arg("generationLabel", generationLabel) }
java { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach { kotlinOptions.jvmTarget = "17" }
]],
  ['app/src/main/kotlin/fixture/Source.kt'] = [[package fixture
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.SOURCE)
annotation class GenerateKsp
@GenerateKsp
@processor.GenerateKapt
class Source
]],
  ['app/src/main/kotlin/fixture/KotlinUse.kt'] = [[package fixture
import generated.KspKotlin
import generated.KspJava
import generated.KaptJava
fun kotlinGenerated() = KspKotlin().value()
fun javaGenerated() = KspJava().value()
fun kaptGenerated() = KaptJava().value()
]],
  ['app/src/main/java/fixture/JavaUse.java'] = [[package fixture;
import generated.KspKotlin;
import generated.KspJava;
import generated.KaptJava;
public class JavaUse {
    public String kotlinGenerated() { return new KspKotlin().value(); }
    public String javaGenerated() { return new KspJava().value(); }
    public String kaptGenerated() { return new KaptJava().value(); }
}
]],
  ['oracle/RuntimeCheck.java'] = [[import fixture.JavaUse;
import fixture.KotlinUseKt;
public class RuntimeCheck {
    public static void main(String[] args) {
        if (args.length != 1 || !args[0].matches("v[12]")) throw new IllegalArgumentException();
        String[] actual = {
            new JavaUse().kotlinGenerated(), new JavaUse().javaGenerated(), new JavaUse().kaptGenerated(),
            KotlinUseKt.kotlinGenerated(), KotlinUseKt.javaGenerated(), KotlinUseKt.kaptGenerated()
        };
        String[] generators = {"ksp-kotlin", "ksp-java", "kapt", "ksp-kotlin", "ksp-java", "kapt"};
        for (int i = 0; i < actual.length; i++) {
            if (!actual[i].equals(generators[i] + "-" + args[0])) throw new AssertionError(i + ": " + actual[i]);
        }
        String present = args[0].equals("v1") ? "versionOne" : "versionTwo";
        String absent = args[0].equals("v1") ? "versionTwo" : "versionOne";
        try {
            for (Class<?> type : new Class<?>[]{generated.KspKotlin.class, generated.KspJava.class, generated.KaptJava.class}) {
                Object instance = type.getConstructor().newInstance();
                if (!type.getMethod(present).invoke(instance).equals(type.getMethod("value").invoke(instance))) throw new AssertionError(type);
                try { type.getMethod(absent); throw new AssertionError("stale API in " + type); }
                catch (NoSuchMethodException expected) {}
            }
        } catch (ReflectiveOperationException failure) { throw new AssertionError(failure); }
        System.out.println("{\"checks\":12,\"pass\":12,\"generation\":\"" + args[0] + "\"}");
    }
}
]],
}
for path, text in pairs(files) do write(root .. '/' .. path, text) end
print('KAPT/KSP fixture: ' .. root)
vim.cmd('qa!')
