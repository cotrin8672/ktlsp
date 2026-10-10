# Exact selected requires-only binaries

These five complete, unmodified JARs total 4,847 bytes. Each contains one
`META-INF/versions/9/module-info.class` and no ordinary class. `hashes.json` records their exact
SHA256 values. Coordinates, Maven source/binary URLs, paired module sources, independent JDK 17
oracle results, and the Apache 2.0 license are preserved in
`crates/ktcore/tests/fixtures/classfile/module-only/` and its parent fixture README.

The upstream projects are Kotlin and kotlinx.coroutines, copyright JetBrains and their
contributors. These inputs are used only as read-only decoder/archive controls. Tests copy them
into disposable directories and never access the user's Gradle cache or the network.

One valid descriptor cannot hide ordinary classes, Kotlin/native metadata, or nested archives.
Unsupported descriptors remain possibly definition-bearing; malformed ZIP reads remain errors.
An empty-surface result is used by the dependency worker only with its existing authoritative,
nonempty exact-selected binary list and before/after origin checks. The tests do not establish
general module semantics, KFF nested-library coverage, or compiler-plugin closure.
