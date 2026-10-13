# Spring overload compiler controls

These standalone reductions accompany the shared inference/selection tests in
implicit_receiver_inference and generic_interop. They use the actual JDK reflect
Type and stdlib List declarations. The Rust index-only facts for mapped List
remain deliberately uncertain until canonical JVM mapping is modeled.

With JDK17 and Kotlin1.9.25 (API/language1.7, JVM17), compile valid/*.java first,
then valid/BeanFactoryExtensions.kt against that output and the Kotlin stdlib.
Inspect oracle.BeanFactoryExtensionsKt using javap -c -s -p:

- getBeanProvider and known invoke BeanFactory.getBeanProvider with
  (Loracle/ResolvableType;)Loracle/ObjectProvider; rather than the Class overload.
- The factory invokes ResolvableType.forType with java.lang.reflect.Type.
- invalid/Rejected.kt fails compilation for the Boolean argument.
- Compile mapped/*.java, then mapped/MappedControl.kt against that output and
  stdlib. Its call selects (Ljava/util/List;)Lmapped/MappedResult;.

Actual native controls used the selected Spring compiler runtime/plugins/options,
with separate temporary outputs. The Java reductions use plain javac -proc:none;
they do not claim ErrorProne/NullAway or full-project/Gradle replay equivalence.
