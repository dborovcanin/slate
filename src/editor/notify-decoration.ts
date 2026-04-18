import {
  Annotation,
  RangeSetBuilder,
  StateEffect,
  StateField,
} from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType } from "@codemirror/view";
import type { NoteReminder } from "../api.ts";
import {
  listNoteReminders,
  markNoteReminderNotified,
  moveNoteReminderLine,
  sendSystemNotification,
} from "../api.ts";
import {
  reconcileReminderLinesOnOpen,
  remindersEqual,
  remapReminderLinesForDocChange,
} from "./reminder-reconcile.ts";

interface ReminderState {
  noteId: string | null;
  remindersByLine: Map<number, NoteReminder>; // 1-based line key
  nowMs: number;
}

const replaceRemindersEffect = StateEffect.define<{
  noteId: string | null;
  reminders: NoteReminder[];
}>();
const upsertReminderEffect = StateEffect.define<{
  noteId: string;
  reminder: NoteReminder;
}>();
const deleteReminderEffect = StateEffect.define<{
  noteId: string;
  lineNumber: number;
}>();
const markReminderNotifiedEffect = StateEffect.define<{
  noteId: string;
  lineNumber: number;
  notifiedAtMs: number;
}>();
const tickReminderNowEffect = StateEffect.define<number>();
const reminderReloadAnnotation = Annotation.define<boolean>();

function reminderRecordEqual(left: NoteReminder, right: NoteReminder): boolean {
  return (
    left.note_id === right.note_id &&
    left.line_number === right.line_number &&
    left.remind_at_ms === right.remind_at_ms &&
    left.display_at === right.display_at &&
    left.line_text === right.line_text &&
    (left.notified_at_ms ?? null) === (right.notified_at_ms ?? null) &&
    left.created_at === right.created_at &&
    left.updated_at === right.updated_at
  );
}

function reminderMapsEqual(
  left: ReadonlyMap<number, NoteReminder>,
  right: ReadonlyMap<number, NoteReminder>,
): boolean {
  if (left.size !== right.size) return false;
  for (const [lineNumber, reminder] of left) {
    const other = right.get(lineNumber);
    if (!other || !reminderRecordEqual(reminder, other)) {
      return false;
    }
  }
  return true;
}

function reminderPersistenceKey(reminder: NoteReminder): string {
  return [
    reminder.note_id,
    reminder.created_at,
    String(reminder.remind_at_ms),
    reminder.display_at,
    reminder.line_text,
    String(reminder.notified_at_ms ?? ""),
  ].join("\u0000");
}

function collectReminderLineMoves(
  previous: ReminderState,
  next: ReminderState,
  doc: { lines: number; line: (lineNumber: number) => { text: string } },
): Array<{ noteId: string; fromLineNumber: number; toLineNumber: number; lineText: string }> {
  if (!previous.noteId || previous.noteId !== next.noteId) {
    return [];
  }

  const previousBuckets = new Map<string, NoteReminder[]>();
  const previousReminders = [...previous.remindersByLine.values()].sort(
    (a, b) => a.line_number - b.line_number,
  );
  for (const reminder of previousReminders) {
    const key = reminderPersistenceKey(reminder);
    const bucket = previousBuckets.get(key);
    if (bucket) {
      bucket.push(reminder);
    } else {
      previousBuckets.set(key, [reminder]);
    }
  }

  const moves: Array<{
    noteId: string;
    fromLineNumber: number;
    toLineNumber: number;
    lineText: string;
  }> = [];
  const nextReminders = [...next.remindersByLine.values()].sort((a, b) => a.line_number - b.line_number);
  for (const reminder of nextReminders) {
    const key = reminderPersistenceKey(reminder);
    const bucket = previousBuckets.get(key);
    if (!bucket || bucket.length === 0) continue;
    const source = bucket.shift();
    if (!source) continue;
    if (source.line_number === reminder.line_number) continue;
    if (reminder.line_number < 1 || reminder.line_number > doc.lines) continue;
    const lineText = doc.line(reminder.line_number).text;
    moves.push({
      noteId: next.noteId,
      fromLineNumber: source.line_number,
      toLineNumber: reminder.line_number,
      lineText,
    });
  }
  return moves;
}

const reminderStateField = StateField.define<ReminderState>({
  create() {
    return {
      noteId: null,
      remindersByLine: new Map(),
      nowMs: Date.now(),
    };
  },
  update(value, tr) {
    let next = value;
    for (const effect of tr.effects) {
      if (effect.is(replaceRemindersEffect)) {
        const remindersByLine = new Map<number, NoteReminder>();
        for (const reminder of effect.value.reminders) {
          remindersByLine.set(reminder.line_number, reminder);
        }
        next = {
          noteId: effect.value.noteId,
          remindersByLine,
          nowMs: Date.now(),
        };
        continue;
      }
      if (effect.is(upsertReminderEffect)) {
        if (next.noteId !== effect.value.noteId) continue;
        const remindersByLine = new Map(next.remindersByLine);
        remindersByLine.set(effect.value.reminder.line_number, effect.value.reminder);
        next = {
          ...next,
          remindersByLine,
        };
        continue;
      }
      if (effect.is(deleteReminderEffect)) {
        if (next.noteId !== effect.value.noteId) continue;
        const remindersByLine = new Map(next.remindersByLine);
        remindersByLine.delete(effect.value.lineNumber);
        next = {
          ...next,
          remindersByLine,
        };
        continue;
      }
      if (effect.is(markReminderNotifiedEffect)) {
        if (next.noteId !== effect.value.noteId) continue;
        const current = next.remindersByLine.get(effect.value.lineNumber);
        if (!current) continue;
        const remindersByLine = new Map(next.remindersByLine);
        remindersByLine.set(effect.value.lineNumber, {
          ...current,
          notified_at_ms: effect.value.notifiedAtMs,
        });
        next = {
          ...next,
          remindersByLine,
        };
        continue;
      }
      if (effect.is(tickReminderNowEffect)) {
        next = {
          ...next,
          nowMs: effect.value,
        };
      }
    }

    if (tr.docChanged && next.noteId && next.remindersByLine.size > 0) {
      const remapped = remapReminderLinesForDocChange(
        next.remindersByLine,
        tr.startState.doc,
        tr.changes,
        tr.newDoc,
      );
      if (!reminderMapsEqual(next.remindersByLine, remapped)) {
        next = {
          ...next,
          remindersByLine: remapped,
        };
      }
    }

    return next;
  },
});

class ReminderGhostWidget extends WidgetType {
  private readonly displayAt: string;
  private readonly expired: boolean;

  constructor(displayAt: string, expired: boolean) {
    super();
    this.displayAt = displayAt;
    this.expired = expired;
  }

  eq(other: ReminderGhostWidget): boolean {
    return other.displayAt === this.displayAt && other.expired === this.expired;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = this.expired ? "notify-ghost notify-ghost-expired" : "notify-ghost";
    span.textContent = `⏰ ${this.displayAt}`;
    return span;
  }
}

const reminderDecorations = EditorView.decorations.compute([reminderStateField], (state) => {
  const reminderState = state.field(reminderStateField, false);
  if (!reminderState || reminderState.remindersByLine.size === 0) {
    return Decoration.none;
  }

  const builder = new RangeSetBuilder<Decoration>();
  const nowMs = reminderState.nowMs;
  for (const [lineNumber, reminder] of reminderState.remindersByLine) {
    if (lineNumber < 1 || lineNumber > state.doc.lines) continue;
    const line = state.doc.line(lineNumber);
    const expired = reminder.remind_at_ms <= nowMs;
    builder.add(
      line.to,
      line.to,
      Decoration.widget({
        widget: new ReminderGhostWidget(reminder.display_at, expired),
        side: 1,
      }),
    );
  }
  return builder.finish();
});

function canUseNotifications(): boolean {
  return typeof window !== "undefined" && typeof Notification !== "undefined";
}

function showReminderToast(body: string): boolean {
  if (typeof document === "undefined") return false;

  let host = document.querySelector(".notify-reminder-toast-host") as HTMLDivElement | null;
  if (!host) {
    host = document.createElement("div");
    host.className = "notify-reminder-toast-host";
    document.body.appendChild(host);
  }

  const toast = document.createElement("div");
  toast.className = "notify-reminder-toast";
  toast.textContent = `⏰ ${body}`;
  host.appendChild(toast);

  window.setTimeout(() => {
    toast.remove();
    if (host && host.childElementCount === 0) {
      host.remove();
    }
  }, 5000);
  return true;
}

async function dispatchReminderNotification(reminder: NoteReminder): Promise<boolean> {
  const body = reminder.line_text.trim().length > 0 ? reminder.line_text : "Reminder";
  let shown = false;
  try {
    await sendSystemNotification("Note reminder", body);
    shown = true;
  } catch (error) {
    console.error("System reminder notification failed:", error);
  }
  if (shown) {
    return true;
  }

  if (!canUseNotifications()) {
    return showReminderToast(body);
  }

  if (Notification.permission === "granted") {
    try {
      new Notification("Note reminder", { body });
      shown = true;
    } catch (error) {
      console.error("Reminder notification failed:", error);
    }
  }
  if (!shown && Notification.permission === "default") {
    try {
      const permission = await Notification.requestPermission();
      if (permission === "granted") {
        new Notification("Note reminder", { body });
        shown = true;
      }
    } catch (error) {
      console.error("Reminder notification permission request failed:", error);
    }
  }

  if (!shown) {
    shown = showReminderToast(body);
  }
  return shown;
}

interface NotifyExtensionOptions {
  getActiveNoteId: () => string | null;
}

function buildReminderPlugin(options: NotifyExtensionOptions) {
  return ViewPlugin.define((view) => {
    let destroyed = false;
    let inFlight = false;
    let pendingReload = false;
    let activeNoteId: string | null = options.getActiveNoteId();
    let loadTimer: number | null = null;
    let tickTimer: number | null = null;
    let lastTickAt = 0;
    let notifying = false;

    function dispatchTick() {
      if (destroyed) return;
      view.dispatch({ effects: [tickReminderNowEffect.of(Date.now())] });
    }

    async function triggerDueReminders(noteId: string) {
      if (notifying) return;
      notifying = true;
      try {
      const state = view.state.field(reminderStateField, false);
      if (!state || state.noteId !== noteId) return;
      const now = Date.now();
      const due = [...state.remindersByLine.values()].filter(
        (reminder) => (reminder.notified_at_ms ?? null) === null && reminder.remind_at_ms <= now,
      );
      if (due.length === 0) return;

      for (const reminder of due) {
        const shown = await dispatchReminderNotification(reminder);
        if (!shown) continue;
        let persistedNotifiedAtMs: number | null = null;
        try {
          const updated = await markNoteReminderNotified(noteId, reminder.line_number, now);
          persistedNotifiedAtMs = updated?.notified_at_ms ?? now;
        } catch (error) {
          console.error("Failed to mark reminder notified:", error);
          continue;
        }
        if (destroyed) return;
        view.dispatch({
          effects: [
            markReminderNotifiedEffect.of({
              noteId,
              lineNumber: reminder.line_number,
              notifiedAtMs: persistedNotifiedAtMs ?? now,
            }),
            tickReminderNowEffect.of(now),
          ],
          annotations: reminderReloadAnnotation.of(true),
        });
      }
      } finally {
        notifying = false;
      }
    }

    async function loadReminders(noteId: string | null) {
      if (inFlight) {
        pendingReload = true;
        return;
      }
      inFlight = true;
      do {
        pendingReload = false;
        try {
          if (!noteId) {
            if (!destroyed) {
              view.dispatch({
                effects: [replaceRemindersEffect.of({ noteId: null, reminders: [] })],
                annotations: reminderReloadAnnotation.of(true),
              });
            }
            continue;
          }
          const reminders = await listNoteReminders(noteId);
          if (destroyed) return;
          const currentNoteId = options.getActiveNoteId();
          if (currentNoteId !== noteId) {
            noteId = currentNoteId;
            pendingReload = true;
            continue;
          }
          view.dispatch({
            effects: [
              replaceRemindersEffect.of({ noteId, reminders }),
              tickReminderNowEffect.of(Date.now()),
            ],
            annotations: reminderReloadAnnotation.of(true),
          });

          void reconcileReminderLinesOnOpen(noteId, reminders, view.state.doc, moveNoteReminderLine)
            .then((reconciled) => {
              if (destroyed) return;
              const latestNoteId = options.getActiveNoteId();
              if (latestNoteId !== noteId) return;
              if (remindersEqual(reminders, reconciled)) return;
              view.dispatch({
                effects: [
                  replaceRemindersEffect.of({ noteId, reminders: reconciled }),
                  tickReminderNowEffect.of(Date.now()),
                ],
                annotations: reminderReloadAnnotation.of(true),
              });
            })
            .catch((error) => {
              console.error("Reminder reconcile load failed:", error);
            });

          await triggerDueReminders(noteId);
        } catch (error) {
          console.error("Reminder load failed:", error);
        }
      } while (pendingReload && !destroyed);
      inFlight = false;
    }

    function scheduleLoad(noteId: string | null) {
      if (loadTimer !== null) {
        window.clearTimeout(loadTimer);
      }
      loadTimer = window.setTimeout(() => {
        void loadReminders(noteId);
      }, 0);
    }

    scheduleLoad(activeNoteId);
    tickTimer = window.setInterval(() => {
      const now = Date.now();
      const noteId = options.getActiveNoteId();
      if (noteId !== activeNoteId) {
        activeNoteId = noteId;
        scheduleLoad(activeNoteId);
      }
      if (now - lastTickAt >= 30_000) {
        lastTickAt = now;
        dispatchTick();
      }
      if (noteId) {
        void triggerDueReminders(noteId);
      }
    }, 1_000);

    return {
      update(update) {
        const nextNoteId = options.getActiveNoteId();
        if (nextNoteId !== activeNoteId) {
          activeNoteId = nextNoteId;
          scheduleLoad(activeNoteId);
        }
        if (
          update.transactions.some((tr) => tr.annotation(reminderReloadAnnotation))
        ) {
          return;
        }
        if (update.docChanged) {
          const previousState = update.startState.field(reminderStateField, false);
          const nextState = update.state.field(reminderStateField, false);
          if (!previousState || !nextState) return;
          const moves = collectReminderLineMoves(previousState, nextState, update.state.doc);
          if (moves.length === 0) return;
          for (const move of moves) {
            void moveNoteReminderLine(
              move.noteId,
              move.fromLineNumber,
              move.toLineNumber,
              move.lineText,
            ).catch((error) => {
              console.error("Failed to persist reminder line move:", error);
            });
          }
        }
      },
      destroy() {
        destroyed = true;
        if (loadTimer !== null) window.clearTimeout(loadTimer);
        if (tickTimer !== null) window.clearInterval(tickTimer);
      },
    };
  });
}

export function notifyExtensions(options: NotifyExtensionOptions) {
  return [
    reminderStateField,
    reminderDecorations,
    buildReminderPlugin(options),
  ];
}

export function applyReminderUpsert(view: EditorView, noteId: string, reminder: NoteReminder) {
  view.dispatch({
    effects: [
      upsertReminderEffect.of({ noteId, reminder }),
      tickReminderNowEffect.of(Date.now()),
    ],
    annotations: reminderReloadAnnotation.of(true),
  });
}

export function applyReminderDelete(view: EditorView, noteId: string, lineNumber: number) {
  view.dispatch({
    effects: [
      deleteReminderEffect.of({ noteId, lineNumber }),
      tickReminderNowEffect.of(Date.now()),
    ],
    annotations: reminderReloadAnnotation.of(true),
  });
}
