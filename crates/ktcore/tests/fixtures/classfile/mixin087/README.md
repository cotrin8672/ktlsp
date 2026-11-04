# Binary annotation identities

`Mixin.class` and `Inject.class` are the original entries
`org/spongepowered/asm/mixin/Mixin.class` and
`org/spongepowered/asm/mixin/injection/Inject.class` from SpongePowered Mixin
0.8.7 (`org.spongepowered:mixin:0.8.7`, MIT license).

They were copied without compiling or modifying bytes from the immutable D9
fixture's `workspace/libs/mixin-0.8.7.jar`. The regression packages only these
classfiles into a disposable jar, with no companion sources. It verifies that
the exact decoded dependency identity can resolve an annotation in incomplete
Java/Kotlin while synthetic previews never become authored source facts.
