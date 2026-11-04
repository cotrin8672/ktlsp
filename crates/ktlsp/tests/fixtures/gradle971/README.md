# Gradle 9.7.1 annotated Java API regression

`Project.class` is the unmodified `org/gradle/api/Project.class` from the Gradle 9.7.1
generated `gradle-api-9.7.1.jar` selected by Square Wire revision
`9a3bce4a6055515b17bb781618409f7703e516ce` during the real Neovim probe.
Only this 14,066-byte declaration fixture is retained; no Gradle installation is
required by the regression test. Gradle is licensed under Apache-2.0; the license
text is included in `LICENSE`.

Class SHA-256: `540be290c5c8a6560a16ebeee86ab31ebf24ac41c1587c28bd14ab7944fd672f`.

The class declares `Project.java` as its SourceFile and carries the invisible
`NotToBeMigratedToLazy` declaration annotation. Its public abstract zero-argument
`getConfigurations` method returns `org.gradle.api.artifacts.ConfigurationContainer`
and carries the visible `HiddenInDefinition` declaration annotation. These Java
declaration annotations must not erase the ordinary getter or its Kotlin property.
Referenced Gradle/Groovy types are not claimed to be present in this fixture.

The test preserves the reported mid-chain cursor: `project.configur|.create(...)`,
including the surrounding plugin class, member property, and override parameter.
It applies the resulting completion edit and checks the restored expression.
