This is a patched local copy of notify 8.2.0.

Source: https://github.com/notify-rs/notify/tree/a1d7c2d8f80786679d58ec6d5986a1d4278bc8cf
Version/tag: notify 8.2.0. License: CC0-1.0, see LICENSE-CC0.
The registry index checksum for the official 8.2.0 .crate is recorded in the delivery receipt;
this directory was copied from the pinned Git source, not from that unpublished-to-this-packet archive.

Only src/windows.rs changes executable source. Its production prefix SHA256 is
509bbbd7cdb0670a9d43dfb13e71bc38432a2c3c73b8fe959764fae382bf7dad.
It adapts official fixes #936, #964, #958, #849, #923 and #906, then adds per-watch ownership
and normal-drop completion drain. The source also includes the focused Windows regression tests.
The matching standalone snapshot passed 14 tests and one separately run real kernel-overflow
test on Windows; see docs/fork-acceptance.md for commands and evidence. These checks do not prove
consumer freshness or an improvement in editor file-watching behavior.

Cargo.toml resolves the original workspace fields locally, rewrites the readme path to README.md,
and replaces the notify-types workspace path with its registry version constraint. It retains
all runtime/feature/target and dev-dependency declarations. The empty local [workspace] allows
the crate's own tests to run separately. Cargo.toml.orig is the original package manifest.
The eight non-Windows src files are byte-identical to the pinned source; Linux/Unix behavior
has not been modified. README.md is copied from the upstream workspace root.

Known limits: callbacks must return without panic; a driver that never completes cancelled I/O
can block drain/join indefinitely. Callback-thread self-drop sends Stop but cannot join itself.
Root rename needs a consumer parent watch. The resolved notify-types 2.1 dependency requires
Rust 1.85 or newer. Unix execution remains untested. Client watch mode is unchanged.
