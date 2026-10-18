# Nonstatic generic Java owners

These actual classfiles were produced with JDK 21 javac, --release 17,
-encoding UTF-8 and -proc:none. Their Java sources are preserved in
../sources/outerfixture and their exact hashes in ../hashes.json.

Outer<T extends Item>.Inner<U extends Other> keeps captured T separate
from U. Factory binds them to SpecialItem and Other. Inner.shadow and
Outer.Shadow deliberately reuse T to check nearest lexical scope.

The original native compiler oracle accepted the valid chain and rejected
the wrong-member calls. It is not a Rust or editor acceptance result.
The portable regressions decode these bytes and exercise shared Java/Kotlin
completion/navigation from the materialized binary declarations.
