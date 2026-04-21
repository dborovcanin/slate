const MAX_COMMAND_HISTORY = 100;

const commandHistory: string[] = [];

function sanitizeCommand(rawCommand: string): string {
  return rawCommand.trim().replace(/^:/, "");
}

export function rememberCommand(rawCommand: string) {
  const command = sanitizeCommand(rawCommand);
  if (!command) return;

  const existing = commandHistory.lastIndexOf(command);
  if (existing >= 0) {
    commandHistory.splice(existing, 1);
  }
  commandHistory.push(command);

  if (commandHistory.length > MAX_COMMAND_HISTORY) {
    commandHistory.splice(0, commandHistory.length - MAX_COMMAND_HISTORY);
  }
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
    if (commandHistory.length === 0) return null;
    if (this.index === null) {
      this.index = commandHistory.length - 1;
    } else {
      this.index = (this.index + commandHistory.length - 1) % commandHistory.length;
    }
    return commandHistory[this.index] ?? null;
  }

  next(): string | null {
    if (commandHistory.length === 0) return null;
    if (this.index === null) {
      this.index = 0;
    } else {
      this.index = (this.index + 1) % commandHistory.length;
    }
    return commandHistory[this.index] ?? null;
  }
}

export function resetCommandHistoryForTests() {
  commandHistory.length = 0;
}

export function getCommandHistoryForTests(): string[] {
  return [...commandHistory];
}
