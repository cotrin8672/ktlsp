# Selected CEM source buffers

These two unmodified Kotlin buffers preserve the original CEM drill superclass
and delegated-interface context. They come from the read-only
CreateEnchantableMachinery working copy selected by `dev/corpus/cem-working-copy.json`.
The regression indexes the three pinned Create classfiles under
`crates/ktcore/tests/fixtures/classfile/create6010`; omitted Minecraft/Create
ancestors deliberately remain unknown. These are source fixtures, not a
standalone build of CEM.

SHA-256 over UTF-8 lines joined with LF, without a terminal LF (the corpus
manifest convention):

- `EnchantableDrillBlockEntity.kt`: `3ef6ba4809e344c17d2c9275005b6c0acdfea496dfbc286160f3bd74a2c53289`
- `EnchantableBlockEntity.kt`: `5dbe4f1a019496492d3c52552808c721c18458cbfbfda61118d6d89a184e523e`

The original buffers and their expectations remain unchanged.

`CemSuperInference120.kt` is a separate authored control. Its abstract class
uses the real Drill constructor and the original CEM interface, avoiding stub
implementations of interface methods. Kotlin 2.4/JDK21 compiles it against the
recovered complete dependency classpath. The existing corpus extra hook opens
it only as a new unsaved buffer and checks the exact `: Float` inlay position
after `speed`; it does not replace either original buffer.
