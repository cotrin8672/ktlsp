import org.gradle.api.initialization.resolve.RepositoriesMode

rootProject.name = "ktlsp-source-encoding-policy"
dependencyResolutionManagement {
    repositoriesMode.set(if (providers.gradleProperty("repositoryPolicy").orNull == "project")
        RepositoriesMode.PREFER_PROJECT else RepositoriesMode.PREFER_SETTINGS)
    repositories { maven { url = uri("settings-repo") } }
}
