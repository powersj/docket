# Reviewer

macOS Dioxus 0.7 desktop app. See README.md for use and DESIGN.md for the design;
DESIGN.md section 15 lists where the build differs from it.

## Before you finish any change

Run `make lint` after every change to Rust code, `Cargo.toml`, or `deny.toml`, and fix
everything it reports before you call the work done. `make fmt` fixes formatting. Then run
`make test`. The git pre-commit hook and CI both run `make lint`, so a change that fails it
cannot merge.

- Don't silence a lint to make it pass. If a lint is wrong for one item, put
  `#[expect(clippy::lint_name, reason = "...")]` on that item. Change crate-wide levels only
  in `[lints]` in `Cargo.toml`, with a comment saying why.
- Every `unsafe` block needs a `// SAFETY:` comment saying why it is sound.
- Snapshot tests use `insta`. When output changes on purpose, run `cargo insta review` and
  check each diff before accepting it. `src/agent/snapshots/` holds the full review prompt.
- Untrusted input boundaries (`markdown::render`, `prompt::fence`, `DiffIndex::parse`) have
  property tests. Keep them passing; add one when you add a boundary.

## Commands

- `make test`: unit, property, and snapshot tests, through nextest (one process per test).
- `cargo test -- --ignored sandbox`: runs the real `claude` CLI to check the sandbox
  (costs a few cents). Run after changing `src/agent/sandbox.rs` or the CLI version.
- `SNAPSHOT_DIR=/tmp/snap cargo test -- --ignored ui_snapshot`, then
  `qlmanage -t -s 2880 -o /tmp/snap /tmp/snap/review.html`: renders the real UI components
  with sample data to a PNG. Use it to check visual changes; the live window can't be
  screenshotted without Screen Recording permission.

## Dioxus pitfalls (each one has bitten this codebase)

- Call hooks (`use_signal`, `use_memo`, `use_context`, …) before any early `return` in a
  component.
- Never write a signal while a `.read()` guard on it is alive. `if let Some(x) =
  sig.read()...` keeps the guard for the whole block; copy the value into a `let` first.
  This compiles and panics at runtime.
- RSX format strings (`"{x}"`) take only simple expressions: no `if`, no method calls with
  string literals. Compute the string beforehand.
- An `if` expression inside a component prop is parsed as a conditional attribute and
  becomes a `String`. Hoist it to a `let`.
- Don't name a local `key` in a component; it clashes with the RSX `key` attribute.

## Things that look wrong but aren't

- Window dragging is native (`src/ui/drag.rs`), not `window().drag()` in an
  `onmousedown`, which arrives too late. Mark draggable areas with `data-drag-region`.
- Vibrancy is applied from a hook in `App`, after the webview exists. Applying it in
  `with_on_window` silently does nothing because wry replaces the content view.
- Skills run by starting the prompt with `/<skill>`; the agent has no Skill tool.
- The agent's cwd is an empty sandbox, so its git rules use `git -C <worktree>`.
