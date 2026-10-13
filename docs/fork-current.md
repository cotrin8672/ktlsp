# Current fork state

Read this before the historical [acceptance record](fork-acceptance.md).
Goal remains in progress. Updated 2026-10-14 (Asia/Tokyo).

- Current implementation: checkpoint76; binary-fields base d3cb519. Use git log -1
  for the latest revision.
- Last complete Rust/release validation: 1,502 passed, 0 failed, 4 ignored.
- File/Module: actual seven-source Gradle builds. Field/method corpus passes cold/warm
  on release75; Module12+2 passes cold/warm again on current release76.
- Current real Neovim: Local library41/41 and Module library41/41. The three earlier
  Module diagnostic omissions are fixed; six first diagnostic reports match per kind
  across unsaved owner/caller changes and restoration.
- Next: final current-binary six-project floor, then remaining diagnostic/compiler/
  live-refresh gaps and controlled startup/index/completion measurements. Reuse
  recorded sources/models and the existing harness; do not call older results current.
- Broader open work: inherited/generated Java diagnostic coverage, compiler/plugin
  adapter fidelity, newest live Gradle refresh, broader UX/IDEA checks, and a final
  same-binary six-project correctness/performance run. Enum/generic binary members open.

## Efficient continuation

Keep raw logs on disk. Read counts/status first; retrieve only relevant failure
snippets or code ranges. Run focused checks during iteration and broad checks after
a settled semantic change. Reuse existing harness/helpers; report unique cases
separately from repeated cold/warm observations. Agents return paths, results and
next action in at most10–15 lines; use sol6.1/high and parallelize independent work.
Preserve prior successful evidence; rerun only when changed code or unresolved
concerns justify it. Source/model completeness, compiler fidelity and performance
are separate claims. Empty results are not successful useful completion.

Local artifacts: C:/Users/gummy/AppData/Local/Temp/ktlsp-selected-compiler-recipe-20261012-1.
Current launch state: current-slice76.json. Latest closed private Gradle-home owner:
native-binary-fields-local-library76-1/receipt.json. No preexisting process adoption
or global process inventory; only fresh owned descendants. Keep AGENTS.md untouched.
