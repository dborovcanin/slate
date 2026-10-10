# Architecture Refactor: Core-Owned Editing

Status: phases 1–8 complete on `session` (2026-10-10). Editing semantics,
history, calc and fold policy, note lifecycle and display semantics live in
the shared crates; `crates/tui` is an input adapter, renderer and effect host.
The canonical boundary is `roadmap/editor-engine-contract.md`.

The full plan, step-by-step execution log and qualification evidence are in
git history: `git show 201bd55:roadmap/arch-refactor.md`.

## Outcome

| Phase | Shared owner |
| --- | --- |
| 1. Buffer primitives | `editor-core`: offsets, text changes, typing, plain/table paste |
| 2. Undo store and policy | `editor-core` history; session supplies grouping and reminder order |
| 3. Word motions | `editor-core`: motions and backward deletion |
| 4. Vim intent execution | `editor-core` plans and registers; session applies visual edits, paste and open-line |
| 5. Post-edit planning | Session: calc evaluation/upkeep and fold structure from exact edit deltas |
| 6. Completion and search | Shared parsing/search; session applies completion edits |
| 7. Shared note session | `note-session`: document, atomic edits/history, calc/folds, lifecycle, save, scripts, rates |
| 8. Shared display model | Session display modules: semantic roles, transforms, source mappings, bounded caches |

Qualification against main `af2a090`: workspace tests, formatting and Clippy
(no new warnings) pass; all original Vim golden cases are identical; paired
large-note and table runs show no metric crossing both investigation
thresholds (p50 +10% and +0.3 ms). Ten live-terminal A/B scenarios match
except the intended Normal-mode undo cursor fix.

## Rules for changes to shared crates

- Keep in-place mutation; no extra clones or allocations on key paths.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
  `cargo fmt --all --check`.
- Golden replays (`crates/editor-core/tests/golden_replay.rs`,
  `crates/tui/src/terminal/app/tests/replay.rs`) pass unchanged unless a
  behavior change is intended and reviewed.
- Run the large-note perf tests (or `perf-check` against
  `perf/baselines/large_note.json`) for changes to editing, calc or folding;
  p50/p95 must not regress. Compare A/B in separate worktrees with separate
  `CARGO_TARGET_DIR`s.
- `note-session` depends only on `editor-core`, `app-core` and
  `table-syntax`, and must not spawn threads or read the clock. Its
  `clippy.toml` disallows those methods and `Cargo.toml` denies
  `clippy::disallowed_methods`.
- Update `roadmap/editor-engine-contract.md` when a contract changes.

## Open follow-ups

- **Startup qualification:** the unchanged absolute startup limits fail on
  this machine for clean main as well. Fix and measure separately.
- **Search allocation:** query changes still lowercase each line; reuse a
  buffer.
- **Job API types:** runner functions in `crates/session/src/jobs.rs` take a
  job enum and `panic!` on the wrong variant; give each runner its own job
  type.
- **Display caches:** the `thread_local!` caches in
  `crates/session/src/display/` clone on every hit and have no byte budget.
- **Edit routing:** `mark_edited_from_line_with_span`
  (`crates/tui/src/terminal/app/editing.rs`) has a `cfg(test)`-only path for
  edits that bypass session transactions; route tests through the session and
  remove it.
- **Session Clippy:** `note-session` still has existing Clippy warnings.
- **Fold view map:** move it to the session when a second consumer or a test
  needs it.

## Before a second front end

- Keep the GUI out of the root workspace at first: its own workspace (listed
  under `exclude`) with path dependencies on the core crates and the session.
  A GPUI git dependency in the root workspace would be fetched by every
  terminal build and CI run. Give it its own CI job with the GPUI system
  libraries.
- GPUI text input uses UTF-16 ranges, marked (IME composition) text and pixel
  bounds. The GUI needs an adapter to the session's character columns and
  byte-offset edits, converted per line, and a decision on how composition
  interacts with undo grouping.
- Decide the editor font early: the display model assumes monospace.
- Qualify input/IME, fonts, pixel layout, accessibility and platform behavior
  before relying on it.
