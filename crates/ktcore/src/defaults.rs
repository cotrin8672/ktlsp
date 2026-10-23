//! Shared Kotlin language defaults used across semantic modules.

/// Packages Kotlin imports implicitly into every file (JVM target). Symbols in these resolve
/// without an explicit `import`.
pub const DEFAULT_IMPORT_PACKAGES: &[&str] = &[
    "kotlin",
    "kotlin.annotation",
    "kotlin.collections",
    "kotlin.comparisons",
    "kotlin.io",
    "kotlin.ranges",
    "kotlin.sequences",
    "kotlin.text",
    "kotlin.jvm",
    "java.lang",
];

/// Whether `pkg` is one of Kotlin's implicit default-import packages.
pub fn is_default_import_pkg(pkg: &str) -> bool {
    DEFAULT_IMPORT_PACKAGES.contains(&pkg)
}

/// Fundamental Kotlin types exist even when no library source declaration is indexed.
pub(crate) fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "Any"
            | "Nothing"
            | "Unit"
            | "Number"
            | "Byte"
            | "Short"
            | "Int"
            | "Long"
            | "Float"
            | "Double"
            | "Boolean"
            | "Char"
            | "String"
            | "CharSequence"
    )
}

pub(crate) fn is_jvm_reference_builtin(name: &str) -> bool {
    matches!(name, "String" | "CharSequence")
}
