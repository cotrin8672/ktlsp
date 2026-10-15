# Binary hierarchy shadow fixtures

These authored Java fixtures are compiled with JDK 21 from
`../sources/shadowfixture/*.java`. They cover abstract re-abstraction,
generic covariant overrides, private and public field hiding, a method
sharing a field's name, and a marker interface without members.

The tests use the compiler-produced classfiles, including covariant bridges,
through local and module binary indexing with cold and warm symbol caches.
