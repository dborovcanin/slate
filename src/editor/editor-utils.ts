import { EditorView } from "@codemirror/view";

export function insertAtSelection(view: EditorView, text: string) {
  const main = view.state.selection.main;
  view.dispatch({
    changes: { from: main.from, to: main.to, insert: text },
    selection: { anchor: main.from + text.length },
    scrollIntoView: true,
  });
}
