# Java source diagnostic reduction

This Java17/Gradle7.4.2 fixture uses the existing corpus harness. It opens only
Use.java; Current.java remains a closed declaration. The first known completion
and exact definition are followed by one unsaved edit, another useful completion
and one standard document diagnostic pull. All source bytes are restored.

The seven semantic expectations remain required. The current release matches
five: useful controls, a full diagnostic report and the absent Java member.
Missing bare-type and import errors still receive no corresponding diagnostics.
These two mismatches are coverage gaps. The harness therefore exits nonzero.
An unanswered or cancelled request is unmeasured, separate from those wrong results.

Use a JDK17 host and target, JAVA_HOME pointing to that JDK, and KTLSP_JDK_SRC
pointing to its lib/src.zip. The selected plain javac task explicitly disables
annotation processing; this case makes no claim about transforming processors.
Copy the fixture before running, keeping generated output outside the checkout:

~~~sh
fixture=$(mktemp -d)
cp -R dev/corpus/fixtures/java-source-diagnostics/. "$fixture/"
cp dev/gradle-wrapper-template/gradlew dev/gradle-wrapper-template/gradlew.bat "$fixture/"
cp dev/gradle-wrapper-template/gradle/wrapper/gradle-wrapper.jar "$fixture/gradle/wrapper/"
KTLSP_CORPUS_WAIT_INDEX=1 \
KTLSP_CORPUS_EXTRA_PROBES="$fixture/java-diagnostic-extra.lua" \
rtk proxy bash -o igncr dev/ktlsp-harness.sh corpus \
  --root "$fixture" --file "$fixture/plain-java.probes.json" --release
~~~

Retain this fixture's Gradle7.4.2 wrapper properties. Configure the JDK17
installation path outside the checkout if auto-discovery selects another JDK.
The recorded comparison uses javac17.0.7 with -source17 -target17 -proc:none.
Independent compilation of the exact edited snapshot reports the absent member,
Missing, missing.Gone and the use of Gone. The hook specifies the edit literally;
it does not replace compiler errors with expected stubs.

IDEA advertises document diagnostics and returns these errors through Neovim's
automatic refresh pull. Its import error covers the missing package qualifier;
the original hook's Gone span remains a calibration mismatch. In the recorded
manual-pull attempt, native Neovim refresh
cancels that request; its four diagnostic checks remain unmeasured. Automatic
results are independent supplemental evidence, not a passing manual request.
See checkpoint48 in docs/fork-acceptance.md for exact artifacts and limitations.

For the default-JDK metadata regression, replace the disposable copy's
build.gradle.kts with metadata-guards.gradle.kts. It uses the same two Java files
and Gradle7.4.2 wrapper, without setting main's toolchain. Run the wrapper with
the repository's scripts/classpath-dump.init.gradle.kts and ktlspDumpClasspath.
The main and apDisabled records must report the actual Gradle JDK with
javaHomeSelection=gradle-current-jvm-7.4.2. Fork mode, fork home, executable,
JVM arguments and the custom JavaCompile subclass must retain unknown homes;
each record must still be complete and have no problem.
An explicit Java toolchain must keep javaHomeSelection=javaCompiler.
The ordinary default configuration now reproduces the same five matches/two
type-import gaps; checkpoint49 records the before/after native runs.
