import {
  VIM_KEY_KIND,
  type VimContext,
  type VimKeyInput,
  type VimSession,
  type VimStep,
} from "./wasm.ts";

export type UiVimEventInput = Pick<
  KeyboardEvent,
  "key" | "code" | "ctrlKey" | "altKey" | "metaKey"
>;

export interface UiVimPipelineContext {
  hasSearchMatches: boolean;
  lineCount: number;
  macroRecording: boolean;
}

export type UiVimPipelineResult =
  | { kind: "no_intent" }
  | { kind: "no_step" }
  | { kind: "handled"; step: VimStep }
  | { kind: "unhandled"; step: VimStep };

function firstCodePoint(value: string): number | null {
  if (!value) return null;
  const codePoint = value.codePointAt(0);
  return typeof codePoint === "number" ? codePoint : null;
}

export function toUiVimKeyInput(event: UiVimEventInput): VimKeyInput | null {
  const key = event.key;
  const code = event.code;

  if (key === "Escape" || key === "Esc" || code === "Escape") {
    return { kind: VIM_KEY_KIND.ESC };
  }
  if (key === "Enter") return { kind: VIM_KEY_KIND.ENTER };
  if (key === "Tab") return { kind: VIM_KEY_KIND.TAB };
  if (key === "Backspace") return { kind: VIM_KEY_KIND.BACKSPACE };
  if (key === "Delete" || key === "Del") return { kind: VIM_KEY_KIND.DELETE };
  if (key === "ArrowUp" || key === "Up" || code === "ArrowUp") {
    return { kind: VIM_KEY_KIND.ARROW_UP };
  }
  if (key === "ArrowDown" || key === "Down" || code === "ArrowDown") {
    return { kind: VIM_KEY_KIND.ARROW_DOWN };
  }
  if (key === "ArrowLeft" || key === "Left" || code === "ArrowLeft") {
    return { kind: VIM_KEY_KIND.ARROW_LEFT };
  }
  if (key === "ArrowRight" || key === "Right" || code === "ArrowRight") {
    return { kind: VIM_KEY_KIND.ARROW_RIGHT };
  }

  const isPlain = !event.ctrlKey && !event.altKey && !event.metaKey;
  if (isPlain && (key === "Home" || code === "Home")) {
    return { kind: VIM_KEY_KIND.CHAR, charCode: "0".charCodeAt(0) };
  }
  if (isPlain && (key === "End" || code === "End")) {
    return { kind: VIM_KEY_KIND.CHAR, charCode: "$".charCodeAt(0) };
  }

  if (event.ctrlKey && !event.altKey && !event.metaKey) {
    const codePoint = firstCodePoint(event.key.toLowerCase());
    if (codePoint !== null) {
      return { kind: VIM_KEY_KIND.CTRL, charCode: codePoint };
    }
    return null;
  }

  if (isPlain && key.length === 1) {
    const codePoint = firstCodePoint(key);
    if (codePoint !== null) {
      return { kind: VIM_KEY_KIND.CHAR, charCode: codePoint };
    }
  }

  return null;
}

export function buildUiVimContext(context: UiVimPipelineContext): VimContext {
  return {
    has_search_matches: context.hasSearchMatches,
    line_count: context.lineCount,
    macro_recording: context.macroRecording,
  };
}

export function runUiVimPipeline(
  session: VimSession,
  event: UiVimEventInput,
  context: UiVimPipelineContext,
): UiVimPipelineResult {
  const keyInput = toUiVimKeyInput(event);
  if (!keyInput) return { kind: "no_intent" };
  const step = session.step(keyInput, buildUiVimContext(context));
  if (!step) return { kind: "no_step" };
  return step.handled ? { kind: "handled", step } : { kind: "unhandled", step };
}
