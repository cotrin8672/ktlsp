plugins { java }

java { toolchain { languageVersion.set(JavaLanguageVersion.of(17)) } }
tasks.withType<org.gradle.api.tasks.compile.JavaCompile>().configureEach { options.compilerArgs.add("-proc:none") }
