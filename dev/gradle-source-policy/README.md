# Selected dependency sources and encoding

This test fixture generates two file-Maven repositories containing the same coordinate.
The settings repository has UTF-8 Java sources and a UTF-8 root POM. The project repository
has ISO-8859-1 sources whose encoding comes from a parent POM outside the dependency graph.
Both contain the Java member `café` and the ASCII control member `known`. Their declaration
lines differ, so navigation must select the expected source representation.

`prepareRepositories` compiles each library with the JDK compiler and its explicit encoding,
then writes the binary JAR, original source JAR, POMs, and UTF-8 navigation oracles. Oracles
live under `build/policy-fixture/oracle`; they are outside the configured source roots and
the corpus harness only reads them as expected results.

Run in disposable copies with a JDK 17+ and an isolated `GRADLE_USER_HOME`. Copy `gradlew`,
`gradlew.bat`, and `gradle/wrapper/gradle-wrapper.jar` from `dev/gradle-wrapper-template` into
each copy; this fixture supplies the Gradle 9.6.1 wrapper properties. An installed Gradle
9.6.1 can run the preparation and direct query without wrapper downloads.

For the settings policy:

```sh
./gradlew --no-daemon --offline prepareRepositories
./gradlew --no-daemon --offline --no-configuration-cache \
  -I /absolute/ktlsp/scripts/classpath-dump.init.gradle.kts ktlspDumpSources
KTLSP_CORPUS_WAIT_INDEX=1 /absolute/ktlsp/dev/ktlsp-harness.sh corpus \
  --root "$PWD" --file "$PWD/settings.json" --bin /absolute/ktlsp --no-build
```

For a second copy, write `repositoryPolicy=project` to its `gradle.properties`, prepare it,
and use `project.json`. Reuse the isolated Gradle home to exercise competing cached
coordinates across repository policies. The direct query must export the selected source
and root POM for both `main` and `test`, plus the project policy's exact parent POM.
The corpus checks ordinary host completion and exact source navigation for both members.
For the project policy, set `KTLSP_CORPUS_EXTRA_PROBES` to this fixture's absolute
`probes.lua` path. The existing corpus hook changes the selected parent encoding to UTF-8,
sends the standard watched-file event, waits for the refreshed dependency generation,
and checks that the old decoded source declaration retracts while the binary `known`
control remains useful. Valid binary completion remains allowed. It restores
the parent, verifies completion and navigation recover, inspects the decoded target text,
and saves `selected-parent-proof.json`. This proves event handling after registration;
it does not measure operating-system file notification latency.

The production parent-discovery guards can also be exercised against the prepared settings
copy:

```sh
./gradlew --no-daemon --offline prepareRepositories
./gradlew --no-daemon --offline --no-configuration-cache \
  -PmetadataGuard=doctype \
  -I /absolute/ktlsp/scripts/classpath-dump.init.gradle.kts ktlspDumpSources
```

Supported guard values are `namespace`, `nested`, `duplicate`, `interpolated`, `dotdot`,
`doctype`, and `oversize`. The fixture first resolves the valid dependency graph, then
changes the selected physical POM before the production artifact query and discovery run.
The source query should remain complete; the POM query should report failure and retain
only its positive root artifact, without fetching a discovered parent. Regenerate the
repositories before the next guard case. These are intentionally invalid metadata cases;
interpolated parent identities remain an analysis coverage gap.

The portable generator and all seven guard cases passed against the pinned production init
script on JDK 21 / Gradle 9.6.1. Each guard retained the positive root POM, reported an
incomplete POM query, and exported no discovered parent. The seven deliberate POM changes
were audited separately; both owned Gradle stages closed with EOF, no forced cleanup, and
no surviving processes. Local evidence is under
`native-gradle-parent-guards87-1/receipt.json` in the selected compiler recipe run directory.

The separate `native-gradle-parent-policy87-2/receipt.json` control verified both repository
policies' actual source/root/parent paths and independent javac/javap `café` oracles.

The official IntelliJ server `ILS-263.6379.0`, through the existing corpus `intellij` mode,
successfully imported and indexed both prepared projects. All eight requests answered:
the UTF-8 case matched both completions and exact source declarations; the inherited ISO
case matched both completions and ASCII source navigation, but `café` navigation returned
the selected binary class instead of its source. The source expectation remains unchanged.
Both comparator processes acknowledged shutdown but exceeded the 16-second exit observation
and required owned cleanup; neither left surviving processes. The requested SDK was JDK 21,
while the observed import Tooling API and Gradle daemon used JDK 22; the imported project SDK
was not independently established. This is semantic comparator evidence, not a matched-SDK
performance comparison. See `native-gradle-parent-idea87-3/receipt.json` and its referenced
corpus results in the selected compiler recipe run directory.

Host parser unit checks passed. The stock release matched all eight baseline completion and
exact source-navigation requests, including inherited-ISO `café`, and shut down cleanly.
The parent-edit and recovery probe is being verified separately.

The actual within-project collision control in `native-gradle-parent-collision87-2/receipt.json`
placed the child only in repository B and conflicting parent POMs in repositories A and B.
An inherited dependency-version property independently showed that Gradle selected parent A;
the production metadata query exported that same parent. This control found no parent-origin
mismatch. No performance comparison or CEM-wide source coverage is claimed here.
