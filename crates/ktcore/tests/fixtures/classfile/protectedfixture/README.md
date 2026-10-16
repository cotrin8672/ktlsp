# Protected binary member fixtures

The authored sources in `../sources/protectedfixture/` compile with JDK 21:

```sh
javac --release 17 -encoding UTF-8 -d /tmp/protected-fixture ../sources/protectedfixture/*.java
```

Copy the resulting `protectedfixture/*.class` files into this directory. The fixture
reduces the selected Create 6.0.10-280 `BlockBreakingKineticBlockEntity.getBreakSpeed`
descriptor `()F` and `ACC_PROTECTED` flag `0x0004`. `transform` independently covers
a protected member with a nominal parameter and return; `known` and `hiddenControl`
are public and private access controls. No game classes or runtime behavior are required.

JDK 21 javac and Kotlin 2.4 independent controls accepted subclass `super` calls and same-package
unrelated access to this Java protected API. Both rejected unrelated foreign-package access,
foreign-package subclass access through a base-typed receiver, and private superclass access.
The eleven owned compiler stages closed with EOF and no surviving or forced-cleanup processes;
see `native-cem-protected87-1/compiler/receipt.json` in the selected compiler recipe run directory.
Kotlin-authored protected visibility is a separate policy; these Java controls do not certify it.
Semantic and editor acceptance is pending; these files do not claim a fix.
