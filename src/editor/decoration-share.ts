import type { EditorView, DecorationSet } from "@codemirror/view";

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
