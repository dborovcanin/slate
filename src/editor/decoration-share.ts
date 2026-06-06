import type { Decoration, EditorView, DecorationSet } from "@codemirror/view";
import type { Range } from "@codemirror/state";

/**
 * Per-view hand-off of the calc decoration set computed by the markdown
 * plugin's combined viewport pass. The markdown plugin (registered first) builds
 * both decoration sets in a single scan and stashes the calc half here; the calc
 * plugin (registered after) reads it in the same dispatch instead of scanning
 * the viewport a second time. Lives in its own module so neither decoration
 * module has to import the other for the hand-off (avoids an import cycle).
 *
 * The entry is tagged with the state + viewport it was built for; the calc
 * plugin only reuses it when both match the current dispatch and otherwise falls
 * back to building its own (so correctness never depends on the stash).
 */
interface StashedCalcDecorations {
  state: EditorView["state"];
  vpFrom: number;
  vpTo: number;
  calc: DecorationSet;
}

const combinedCalcStash = new WeakMap<EditorView, StashedCalcDecorations>();

export function stashCombinedCalcDecorations(view: EditorView, calc: DecorationSet): void {
  combinedCalcStash.set(view, {
    state: view.state,
    vpFrom: view.viewport.from,
    vpTo: view.viewport.to,
    calc,
  });
}

/**
 * Replace every decoration overlapping the byte range [fromPos, toPos] in `base`
 * with the decorations `rebuilt` carries in that same range. Used by the
 * changed-line delta path: `base` is the previous set already mapped through the
 * edit (so unchanged lines are correct), and `rebuilt` is a fresh build of just
 * the changed line span. Markdown/calc decorations are per-line (none span the
 * region boundary), so a flat filter+add splice is exact.
 */
export function spliceDecorations(
  base: DecorationSet,
  fromPos: number,
  toPos: number,
  rebuilt: DecorationSet,
): DecorationSet {
  const add: Range<Decoration>[] = [];
  rebuilt.between(fromPos, toPos, (from, to, value) => {
    add.push(value.range(from, to));
  });
  return base.update({
    filterFrom: fromPos,
    filterTo: toPos,
    filter: () => false,
    add,
    sort: true,
  });
}

export function takeFreshCombinedCalcDecorations(view: EditorView): DecorationSet | null {
  const stash = combinedCalcStash.get(view);
  if (
    stash &&
    stash.state === view.state &&
    stash.vpFrom === view.viewport.from &&
    stash.vpTo === view.viewport.to
  ) {
    return stash.calc;
  }
  return null;
}
