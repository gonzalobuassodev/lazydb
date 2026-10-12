# Persistent Leader Menu Implementation Plan

**Goal:** Keep Space leader hints visible until completion, cancellation, or a
context change, while retaining the configured timeout for ordinary sequences.

**Architecture:** Classify leader pending states separately in the existing
keymap. Keep editor leader bindings synchronized with cancellation and context
changes without resetting unrelated Vim interactions.

**Tech Stack:** Rust, crossterm, existing keymap and editor unit/integration tests.

1. Add regressions in `src/input/keymap.rs` and `tests/keymap.rs` for idle
   leader menus, delayed selection, cancellation, nested transaction leaders,
   focus/tab/overlay changes, and unchanged ordinary sequence expiry.
   Run `cargo test --lib leader_menu` and `cargo test --test keymap leader_menu`
   to establish the failing behavior.
2. Update `src/input/keymap.rs`, `src/editor/mod.rs`, `src/app.rs`, and
   `src/runtime.rs` to implement persistent leaders and synchronized clearing.
   Preserve Insert/Replace text input and ordinary sequence timeout behavior.
3. Update `config/default.toml`, `docs/keybindings.md`, and
   `docs/configuration.md` to explain the scope of `sequence_timeout_ms`.
4. Run targeted editor/keymap/help/UI tests, `cargo fmt --check`, and the
   repository's Clippy and all-targets/all-features checks. Review the diff.

No commit or user configuration changes are part of this task.
