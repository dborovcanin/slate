import {
  cycleCommandHistoryNext,
  cycleCommandHistoryPrev,
  rememberCommandHistory,
} from "./wasm.ts";

const MAX_COMMAND_HISTORY = 100;

const commandHistory: string[] = [];

function replaceHistory(next: readonly string[]) {
  commandHistory.splice(0, commandHistory.length, ...next);
}

export function rememberCommand(rawCommand: string) {
  replaceHistory(rememberCommandHistory(commandHistory, rawCommand, MAX_COMMAND_HISTORY));
}

export class CommandHistoryNavigator {
  private index: number | null = null;

  reset() {
    this.index = null;
  }

  isActive(): boolean {
    return this.index !== null;
  }

  previous(): string | null {
    const step = cycleCommandHistoryPrev(commandHistory, this.index);
    if (!step) return null;
    this.index = step.index;
    return step.command;
  }

  next(): string | null {
    const step = cycleCommandHistoryNext(commandHistory, this.index);
    if (!step) return null;
    this.index = step.index;
    return step.command;
  }
}

export function resetCommandHistoryForTests() {
  commandHistory.length = 0;
}

export function getCommandHistoryForTests(): string[] {
  return [...commandHistory];
}
