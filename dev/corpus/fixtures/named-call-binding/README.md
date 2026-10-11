# Named Kotlin calls across Java/Kotlin files

This disposable Gradle 8.7/Kotlin 1.9.23 project checks reordered arguments,
generic overloads, omitted defaults, and exact Kotlin/Java declaration navigation.
It uses the existing corpus harness and an extra probe hook.

The manifest makes 30 first requests and checks 39 semantic results. The hook
makes 17 requests and checks 26 results after one-file unsaved edits, restoration,
and a genuinely ambiguous call. It never retries for desired content. Empty
positive results are coverage gaps; unanswered requests are unmeasured.

Use a JDK17 Gradle host and a JDK8 toolchain. Set JAVA_HOME to JDK17,
IJLS_JDK to the JDK8 home, and KTLSP_JDK_SRC to its src.zip. Configure the
JDK8 toolchain path if Gradle cannot discover it. Prepare a temporary checkout
with the existing wrapper, keeping generated files outside the repository:

```sh
fixture=$(mktemp -d)
cp -R dev/corpus/fixtures/named-call-binding/. "$fixture/"
cp dev/gradle-wrapper-template/gradlew dev/gradle-wrapper-template/gradlew.bat "$fixture/"
cp dev/gradle-wrapper-template/gradle/wrapper/gradle-wrapper.jar "$fixture/gradle/wrapper/"
KTLSP_CORPUS_WAIT_INDEX=1 \
KTLSP_CORPUS_EXTRA_PROBES="$fixture/nvim_named_call_probes.lua" \
rtk proxy bash -o igncr dev/ktlsp-harness.sh corpus \
  --root "$fixture" --file "$fixture/named-calls.json" --release
```

Set KTLSP_PROBE_SERVER=intellij to compare the same expectations through the
installed intellij-lsp plugin. Keep a separate run directory and cache per server.

Independent Kotlin/javac controls compiled the original and matching-caller
states. Four explicit Invocation result assignments also compiled, including
default omission; the editor sources use inferred results. Declaration-only
renaming fails at first=left and reports the missing chosen parameter.
The ambiguous buffer fails with overload resolution ambiguity. To repeat the
positive type control, replace src/main/kotlin/probe/NamesUse.kt in a separate
compiler copy with compiler-oracles/NamesUse-typed.kt and run the wrapper's classes
task. The editing hook records the exact declaration-only and ambiguity mutations.
IDEA and the current ktlsp release matched all 65 semantic checks on the same sources; its shutdown observation is recorded
separately in docs/fork-acceptance.md.

The hook writes request responses, delivered document versions, whole-buffer
snapshots and invalid-code completion policy observations under artifacts/.
It restores both edited buffers and verifies unchanged disk sources. Manual
parameter-name edits are separate from LSP rename coverage. This reduction
does not establish whole-project IDEA parity or a performance result.
