import type { EditorView } from "@codemirror/view";
import { ResolvedContext } from "./core/context.ts";
import { applyEditOperations, snapshotFromView } from "./core/codemirror-adapter.ts";
import {
  executeSumCommand,
  parseNumbers,
  parseScope,
  resolveScopeRange,
  resolveScopeRangeInContext,
  type LineRange,
  type SumScope,
} from "./core/sum.ts";

export type { SumScope, LineRange };
export { parseNumbers, parseScope, resolveScopeRange };

function resolveContextFromView(view: EditorView): ResolvedContext {
  return new ResolvedContext(snapshotFromView(view));
}

export function resolveScopeRangeInView(view: EditorView, scope: SumScope): LineRange | null {
  return resolveScopeRangeInContext(resolveContextFromView(view), scope);
}

async function copyText(text: string) {
  if (typeof navigator === "undefined" || !navigator.clipboard) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch (err) {
    console.error("Clipboard write failed:", err);
  }
}

export async function executeExCommand(view: EditorView, rawCommand: string): Promise<string> {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  if (!trimmed) return "";

  const ctx = resolveContextFromView(view);
  const result = executeSumCommand(trimmed, ctx);

  if (result.operation) {
    applyEditOperations(view, [result.operation]);
  }
  if (result.clipboardText) {
    await copyText(result.clipboardText);
  }

  return result.message;
}
