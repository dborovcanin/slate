# Architecture and Performance Review — Deferred Work

Working list of architecture/performance items that survived the most recent review pass. Completed items are removed; execution backlog lives in `roadmap/plan.md`.

## Document model is a flat `Vec<String>` (architectural observation, monitor)

**Observation**
The TUI's canonical document model is `lines: Vec<String>` (`mod.rs:396`) with a derived `joined_text_cache`. Mid-document structural edits (Enter / join / delete-line) are `Vec::insert` / `Vec::remove` = O(n) pointer memmove of the line vector, and several subsystems are built around the same line-vector assumption (undo prefix/suffix diff, calc `line_metadata` splice, fence checkpoints, folding maps). "Large notes stay fast" is enforced by **tiered feature reduction** (`LARGE_DOC_CALC_DEFER_LINES = 20_000`, `LARGE_NOTE_FULL_FEATURE_LINE_LIMIT = 30_000`, reduced-undo at 30_001) rather than by a sublinear data structure.

This is most likely a deliberate, accepted tradeoff: the line-vector keeps every per-line subsystem simple, and the tier thresholds cap the O(n) costs. No action is proposed now — but it is the structural reason the undo span fast path and the calc-splice logic exist, and it is the ceiling the `30k/100k/200k/400k` regression gates in `plan.md` are defending.

**If a tier gate regresses**, the options in priority order are: (a) widen the span/incremental fast paths so they cover all sizes (calc splice, undo span path) before touching the data model; (b) only if line-vector memmove itself shows up in profiles, consider a gap-buffer-of-lines or rope-of-lines for the TUI model. Option (b) is a large refactor touching every subsystem listed above and should not be undertaken speculatively.
