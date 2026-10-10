-- Disposable fixture shared by the ktlsp and IntelliJ harness modes.
local root = assert(arg[1], 'usage: nvim -l dev/mixed_project.lua <project>')
local function write(path, text)
  local full = root .. '/' .. path
  vim.fn.mkdir(vim.fs.dirname(full), 'p')
  vim.fn.writefile(vim.split(text, '\n', { plain = true, trimempty = false }), full)
end

write('pom.xml', [[<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd">
  <modelVersion>4.0.0</modelVersion>
  <groupId>dev.ktlsp</groupId>
  <artifactId>mixed-harness</artifactId>
  <version>1.0-SNAPSHOT</version>
  <properties>
    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
    <kotlin.version>2.0.21</kotlin.version>
    <maven.compiler.release>17</maven.compiler.release>
  </properties>
  <dependencies>
    <dependency>
      <groupId>org.jetbrains.kotlin</groupId>
      <artifactId>kotlin-stdlib</artifactId>
      <version>${kotlin.version}</version>
    </dependency>
  </dependencies>
  <build>
    <plugins>
      <plugin>
        <groupId>org.jetbrains.kotlin</groupId>
        <artifactId>kotlin-maven-plugin</artifactId>
        <version>${kotlin.version}</version>
        <executions>
          <execution>
            <id>kotlin-compile</id>
            <phase>process-sources</phase>
            <goals><goal>compile</goal></goals>
            <configuration>
              <jvmTarget>17</jvmTarget>
              <sourceDirs>
                <sourceDir>${project.basedir}/src/main/kotlin</sourceDir>
                <sourceDir>${project.basedir}/src/main/java</sourceDir>
              </sourceDirs>
            </configuration>
          </execution>
        </executions>
      </plugin>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-compiler-plugin</artifactId>
        <version>3.13.0</version>
      </plugin>
    </plugins>
  </build>
</project>]])

write('src/main/java/mixed/JavaBox.java', [[package mixed;

public final class JavaBox<T> {
    private final T value;

    public JavaBox(T value) {
        this.value = value;
    }

    public T value() {
        return value;
    }
}]])

write('src/main/java/mixed/JavaApi.java', [[package mixed;

public class JavaApi {
    public String javaGreeting() {
        return "java";
    }

    public KotlinApi kotlinApi() {
        return new KotlinApi(this);
    }

    public String useKotlin(KotlinApi api) {
        return api.kotlinGreeting();
    }

    public String useBox(JavaBox<KotlinApi> box) {
        return box.value().kotlinGreeting();
    }
}]])

write('src/main/kotlin/mixed/KotlinApi.kt', [[package mixed

class KotlinApi(private val peer: JavaApi) {
    fun kotlinGreeting(): String = peer.javaGreeting()

    fun useBox(box: JavaBox<JavaApi>): String = box.value().javaGreeting()
}]])
