// Turn events to chat rows: a pure function, so a turn that is replayed from
// its beginning (after the app was closed) rebuilds exactly the same rows.
import type { Answer, TurnEvent, TurnTokens } from '@/link/types';

import {
  answerLine,
  type AssistantMessage,
  type Message,
  type NoteMessage,
  type NoticeMessage,
  type QuestionMessage,
  type ToolMessage,
} from './model';

export type TurnState = {
  turnId: string;
  /** The rows of this turn, oldest first. Their ids derive from `turnId`. */
  items: Message[];
  /** The assistant row being written, if any. */
  draftId: string | null;
  /** Rows made so far: the source of the ids. */
  made: number;
  /** The agent's conversation, to continue it with the next turn. */
  sessionId?: string;
  model?: string;
  /** The agent's permission mode, once it said. */
  mode?: string;
  /**
   * `running` until the agent finishes; `ended` once nothing more can come.
   * An agent told more while it worked finishes once per thing it was told,
   * and is `running` again in between.
   */
  status: 'running' | 'finished' | 'ended';
  ok?: boolean;
  costUsd?: number;
  durationMs?: number;
  /** The tokens it used, as the agent last said. */
  tokens?: TurnTokens;
  /** The connection dropped; the turn goes on over there. */
  disconnected: boolean;
};

/** The label of the note that holds what the agent thinks. */
export const THINKING = 'thinking';

export function initialTurn(turnId: string): TurnState {
  return { turnId, items: [], draftId: null, made: 0, status: 'running', disconnected: false };
}

function push(state: TurnState, make: (id: string) => Message): TurnState {
  const id = `${state.turnId}:${state.made}`;
  return { ...state, items: [...state.items, make(id)], made: state.made + 1 };
}

function patch(state: TurnState, id: string, change: (message: Message) => Message): TurnState {
  return { ...state, items: state.items.map((m) => (m.id === id ? change(m) : m)) };
}

/** Closes the row being written, keeping what it has. */
function settleDraft(state: TurnState): TurnState {
  if (!state.draftId) return state;
  const settled = patch(state, state.draftId, (m) =>
    m.kind === 'assistant' ? { ...m, streaming: false } : m,
  );
  return { ...settled, draftId: null };
}

function notice(
  state: TurnState,
  tone: NoticeMessage['tone'],
  text: string,
  now: number,
  deniedTools?: string[],
): TurnState {
  return push(state, (id) => ({ id, kind: 'notice', tone, text, at: now, deniedTools }));
}

function deniedText(tools: string[]): string {
  const names = [...new Set(tools)];
  return names.length === 1
    ? `The agent was not allowed to use ${names[0]}.`
    : `The agent was not allowed to use ${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}.`;
}

/** The questions the agent still waits on, oldest first: open or being sent. */
export function pendingQuestions(state: TurnState | undefined): QuestionMessage[] {
  if (!state) return [];
  return state.items.filter(
    (m): m is QuestionMessage =>
      m.kind === 'question' && (m.status === 'open' || m.status === 'sending'),
  );
}

/** Whether the agent waits for the person: a question nobody answered yet. */
export function isWaiting(state: TurnState | undefined): boolean {
  return state?.items.some((m) => m.kind === 'question' && m.status === 'open') ?? false;
}

/** The state after the person answered a question here; the agent may not have it yet. */
export function markAnswered(state: TurnState, questionId: string, answer: Answer): TurnState {
  return {
    ...state,
    items: state.items.map((m) =>
      m.kind === 'question' && m.questionId === questionId && m.status !== 'answered' && m.status !== 'closed'
        ? { ...m, status: 'sending', chosen: answerLine(m.ask, answer) }
        : m,
    ),
  };
}

/** The state after the person said more while the agent works. */
export function sayMore(state: TurnState, text: string, now: number): TurnState {
  return push(settleDraft(state), (id) => ({ id, kind: 'user', text, at: now }));
}

/** More is coming after a `finished`: the agent had been told more. */
function resumed(state: TurnState): TurnState {
  return state.status === 'finished' ? { ...state, status: 'running' } : state;
}

/** The state after `event`. `now` is the time to stamp new rows with. */
export function reduceTurn(before: TurnState, event: TurnEvent, now: number): TurnState {
  const state =
    event.type === 'text-delta' ||
    event.type === 'text' ||
    event.type === 'thought' ||
    event.type === 'tool-use' ||
    event.type === 'question'
      ? resumed(before)
      : before;
  switch (event.type) {
    case 'started':
      return { ...state, sessionId: event.sessionId, model: event.model ?? state.model };

    case 'mode-changed':
      return { ...state, mode: event.mode };

    case 'question': {
      // Asked again (a replay after a gap): the one already shown stands.
      const known = state.items.some(
        (m) => m.kind === 'question' && m.questionId === event.id && m.status !== 'closed',
      );
      if (known) return state;
      return push(settleDraft(state), (id): QuestionMessage => ({
        id,
        kind: 'question',
        questionId: event.id,
        ask: event.ask,
        status: 'open',
        at: now,
      }));
    }

    case 'question-resolved':
      return {
        ...state,
        items: state.items.map((m) =>
          m.kind === 'question' &&
          m.questionId === event.id &&
          (m.status === 'open' || m.status === 'sending')
            ? // Answered from here, or the agent went on without it (answered
              // on that computer, withdrawn, the turn ended).
              { ...m, status: m.status === 'sending' ? 'answered' : 'closed' }
            : m,
        ),
      };

    case 'text-delta': {
      if (state.draftId) {
        return patch(state, state.draftId, (m) =>
          m.kind === 'assistant' ? { ...m, text: m.text + event.text } : m,
        );
      }
      const next = push(state, (id) => ({
        id,
        kind: 'assistant',
        text: event.text,
        at: now,
        streaming: true,
      }));
      return { ...next, draftId: next.items[next.items.length - 1]!.id };
    }

    case 'text': {
      if (state.draftId) {
        const replaced = patch(state, state.draftId, (m) =>
          m.kind === 'assistant' ? { ...m, text: event.text, streaming: false } : m,
        );
        return { ...replaced, draftId: null };
      }
      return push(state, (id): AssistantMessage => ({
        id,
        kind: 'assistant',
        text: event.text,
        at: now,
        streaming: false,
      }));
    }

    case 'thought': {
      // What it thinks is a note of its own: hidden with the other notes,
      // one row for as long as nothing else comes in between.
      const last = state.items[state.items.length - 1];
      if (last?.kind === 'note' && last.label === THINKING) {
        return patch(state, last.id, (m) =>
          m.kind === 'note' ? { ...m, text: m.text + event.text } : m,
        );
      }
      return push(settleDraft(state), (id): NoteMessage => ({
        id,
        kind: 'note',
        label: THINKING,
        text: event.text,
        at: now,
      }));
    }

    case 'tool-use':
      return push(settleDraft(state), (id): ToolMessage => ({
        id,
        kind: 'tool',
        toolId: event.id,
        name: event.name,
        summary: event.summary,
        status: 'running',
        ...(event.detail ? { detail: event.detail } : null),
        at: now,
      }));

    case 'tool-result': {
      const target = [...state.items]
        .reverse()
        .find((m) => m.kind === 'tool' && m.toolId === event.id);
      if (!target) return state;
      return patch(state, target.id, (m) =>
        m.kind === 'tool'
          ? {
              ...m,
              status: event.ok ? 'ok' : 'error',
              result: event.summary || undefined,
              ...(event.detail ? { output: event.detail } : null),
            }
          : m,
      );
    }

    case 'finished': {
      // Exactly one is expected; a second one (a replay glitch) changes nothing.
      if (state.status !== 'running') return state;
      let next: TurnState = {
        ...settleDraft(state),
        status: 'finished',
        ok: event.ok,
        costUsd: event.costUsd,
        durationMs: event.durationMs,
      };
      if (event.deniedTools.length > 0) {
        next = notice(next, 'warning', deniedText(event.deniedTools), now, [
          ...new Set(event.deniedTools),
        ]);
      }
      if (event.interrupted) {
        next = notice(next, 'info', 'Stopped.', now);
      } else if (!event.ok) {
        next = notice(next, 'error', event.error?.trim() || 'The agent stopped with an error.', now);
      }
      return next;
    }

    case 'usage':
      return { ...state, tokens: event.tokens };

    case 'disconnected':
      return { ...state, disconnected: true };

    case 'reconnected':
      return { ...state, disconnected: false };

    case 'gap':
      return notice(
        state,
        'info',
        'Part of this answer is missing: it was longer than the other computer keeps.',
        now,
      );

    case 'exited':
    case 'gone': {
      if (state.status === 'ended') return state;
      let next = settleDraft(state);
      next = {
        ...next,
        items: next.items.map((m) =>
          m.kind === 'tool' && m.status === 'running'
            ? { ...m, status: 'stopped' }
            : m.kind === 'question' && (m.status === 'open' || m.status === 'sending')
              ? { ...m, status: 'closed' }
              : m,
        ),
      };
      if (state.status === 'running') {
        // No `finished` came first: the turn was stopped or its terminal is gone.
        next = notice(
          next,
          event.type === 'gone' ? 'warning' : 'info',
          event.type === 'gone'
            ? 'The other computer no longer has this answer.'
            : 'Stopped.',
          now,
        );
      }
      return { ...next, status: 'ended', disconnected: false };
    }
  }
}

/** Whether nothing more can come from the turn. */
export function isOver(state: TurnState): boolean {
  return state.status === 'ended';
}
