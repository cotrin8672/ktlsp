`Model.kt`, `Model.class`, and `Model$Companion.class` are a Kotlin 1.9.23 serialization
compiler control. The classes were emitted from the unchanged source by the selected
compiler's `GenerationState` / `ClassBuilderMode.KAPT3` header factory, with the actual
Gradle-selected plugin jars and arguments from `dev/compiler_plugin_project.lua`.
They are declaration metadata fixtures, not executable runtime classes.

The companion's generated `serializer()` has descriptor
`()Lkotlinx/serialization/KSerializer;` and checked Signature
`()Lkotlinx/serialization/KSerializer<Lpluginfixture/Model;>;`. The parent owns the
public static final companion field, and both classes record the same InnerClasses entry.

The portable editor reproduction is the unchanged `serialization-*` cases in
`dev/corpus/compiler-plugins.json`, run through `dev/ktlsp-harness.sh corpus` against
the compiler-plugin project. The unit test checks shared normalized facts and host
body invalidation; the editor corpus separately checks compiler/editor integration.

`ModelFull.class` and `CompanionFull.class` are the same selected 1.9.23 compiler's
FULL backend output, including actual plugin lowering. `JavaUse.java` and `JavaUse.class`
are the unchanged Java caller and fresh selected JDK 17 javac output, with the actual
Lombok 1.18.32 processor path and captured release-17 arguments. The compiler control
compiled all three original Java inputs against the current Kotlin output and proved
the no-arg constructor independently. These bytes exercise the host snapshot route,
including Java parser selection and preserving Kotlin API across Java body edits.

SHA-256:

- ModelFull.class: `a73762ca487a2639f4e92ef61488fac5966e303050039d1cd5a462a3c21552ee`
- CompanionFull.class: `a5afbd82466d1b8d2a40f8c2517c796954bc69b2edc7428aac1de81656da4ee2`
- JavaUse.class: `50742274230d2112ee63fd9673a7d71c56a5a651ec90fd7e6b13111dc5e871cd`
- JavaUse.java: `d7013d3fa18caf430f4c4698bee8fcf86d8417851b85c8bb547b5a00a932884c`
