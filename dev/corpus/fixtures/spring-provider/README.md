# Spring provider-chain reduction

This small Java/Kotlin project reduces Spring Framework 6.2.0's `BeanFactory` extensions
(revision `5024bb72279188f1d0dcfe19e8abdd4bdb9887c8`). It exercises reified extension
receivers, Java generic overloads, intermediate selectors and unsaved trailing dots.
It is ordinary language analysis; no framework extension is required.

The six source files are unchanged from the independently compiled reduction.
The reference uses JDK 21 `javac --release 17`, Kotlin compiler 1.9.23 with JVM target 17,
then compiles `JavaUse.java` against the result and checks it with `javap -p -s -c`.
All four stages pass. Kotlin compiler runtime dependencies are stdlib/script-runtime
1.9.23, reflect 1.6.10, trove4j 1.0.20200330 and annotations 13.0.

Use the existing Neovim harness from the repository root:

```sh
KTLSP_CORPUS_WAIT_INDEX=1 dev/ktlsp-harness.sh corpus \
  --root "$PWD/dev/corpus/fixtures/spring-provider" \
  --file "$PWD/dev/corpus/spring-provider-reduction.json" --release
KTLSP_CORPUS_WAIT_INDEX=1 dev/ktlsp-harness.sh corpus \
  --root "$PWD/dev/corpus/fixtures/spring-provider" \
  --file "$PWD/dev/corpus/spring-provider-reduction-incomplete.json" --release
```

Run through RTK; use Git Bash on Windows. The harness isolates its cache and artifacts.
The golden manifest checks exact Java/Kotlin definitions as well as useful completion.
The incomplete manifest edits and restores Neovim buffers without changing disk files.
`crates/ktlsp/tests/implicit_receiver_inference.rs` also covers closed files, module
visibility changes and ambiguous foreign extensions; `extension_host.rs` checks that
recovery preserves a source-provider binding after the completion cursor.

For the full pinned Spring checkout, use `dev/corpus/spring-extensions-editing.json`
with the same corpus route. It keeps the original two controls and adds four unsaved
cases. Its first-response semantics pass; missing dependency sources and shutdown
remain operational limitations. See checkpoint 29 in `docs/fork-acceptance.md` for evidence.
