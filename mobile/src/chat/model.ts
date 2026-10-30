// What a chat shows: one row per thing that was said or done.
import type { AgentOptions, Answer, QuestionKind, TurnOptions } from '@/link/types';

export type UserMessage = { id: string; kind: 'user'; text: string; at: number };

export type AssistantMessage = {
  id: string;
  kind: 'assistant';
  text: string;
  at: number;
  /** Still being written. */
  streaming: boolean;
};

export type ToolMessage = {
  id: string;
  kind: 'tool';
  toolId: string;
  name: string;
  summary: string;
  /** `stopped`: the turn ended before the tool reported. */
  status: 'running' | 'ok' | 'error' | 'stopped';
  /** One line of what came back. */
  result?: string;
  /** What it was called with, in full: the whole command, the edit as a diff. */
  detail?: string;
  /** What came back, in full. */
  output?: string;
  at: number;
};

export type NoticeMessage = {
  id: string;
  kind: 'notice';
  tone: 'info' | 'warning' | 'error';
  text: string;
  at: number;
  /** Tools the agent was refused: the chat offers to ask again with more allowed. */
  deniedTools?: string[];
};

/**
 * Text a program put in the conversation, not a person and not the agent: a
 * task notification, a reminder of the harness, a command the person ran in
 * the agent's own prompt and what it printed. Hidden unless asked for, and
 * then one line until it is opened.
 */
export type NoteMessage = {
  id: string;
  kind: 'note';
  /** What it is, in a word or two: `task notification`, `command output`. */
  label: string;
  text: string;
  at: number;
};

/**
 * Something the agent asked and waits for. `open`: nobody answered yet.
 * `sending`: answered here, the agent does not have it yet (it is kept, and
 * sent when the connection allows). `answered`: the agent has it. `closed`:
 * the agent stopped waiting without an answer from here.
 */
export type QuestionMessage = {
  id: string;
  kind: 'question';
  questionId: string;
  ask: QuestionKind;
  status: 'open' | 'sending' | 'answered' | 'closed';
  /** What was answered, in a line. */
  chosen?: string;
  at: number;
};

export type TodoStatus = 'pending' | 'in-progress' | 'completed';

export type Todo = { text: string; status: TodoStatus };

/**
 * The agent's list of what it means to do, with how far it is. One row per
 * turn: each list the agent writes replaces the one before.
 */
export type TodosMessage = { id: string; kind: 'todos'; todos: Todo[]; at: number };

/** The tool the host sends the agent's list as, and the id it keeps for it. */
export const TODOS_TOOL = 'TodoWrite';
export const TODOS_ID = 'leon:todos';

/** The list in the lines the host writes: `[x] done`, `[~] doing`, `[ ] to do`. */
export function parseTodos(detail: string): Todo[] {
  const todos: Todo[] = [];
  for (const line of detail.split('\n')) {
    const found = /^\[([ x~])\] (.+)$/.exec(line.trimEnd());
    if (!found) continue;
    const mark = found[1];
    todos.push({
      text: found[2]!,
      status: mark === 'x' ? 'completed' : mark === '~' ? 'in-progress' : 'pending',
    });
  }
  return todos;
}

export type Message =
  | UserMessage
  | AssistantMessage
  | ToolMessage
  | NoticeMessage
  | NoteMessage
  | QuestionMessage
  | TodosMessage;

/** A line for what was asked: the tool and what with, the first question, "a plan". */
export function questionLine(ask: QuestionKind): string {
  switch (ask.kind) {
    case 'permission':
      return ask.summary ? `${ask.tool}: ${ask.summary}` : ask.tool;
    case 'choices':
      return ask.questions[0]?.question ?? 'A question';
    case 'plan':
      return 'A plan to approve';
  }
}

/** A line for what was answered, to keep in the chat. */
export function answerLine(ask: QuestionKind, answer: Answer): string {
  if (answer.kind === 'permission' && ask.kind === 'permission') {
    const choice = ask.choices.find((c) => c.id === answer.choiceId);
    const said = choice?.label ?? (answer.choiceId === 'deny' ? 'Deny' : 'Allow');
    return answer.reason?.trim() ? `${said}: ${answer.reason.trim()}` : said;
  }
  if (answer.kind === 'choices') {
    return answer.answers
      .map((a) => [...a.labels, ...(a.other?.trim() ? [a.other.trim()] : [])].join(', '))
      .join(' · ');
  }
  if (answer.kind === 'plan') {
    if (answer.approve) return 'Approved';
    return answer.feedback?.trim() ? `Keep planning: ${answer.feedback.trim()}` : 'Keep planning';
  }
  return answer.kind === 'permission' && answer.choiceId === 'deny' ? 'Declined' : 'Answered';
}

/** The mode's own name, or its id for one this app was not told about. */
export function modeLabel(options: AgentOptions | undefined, mode: string | undefined): string {
  if (!mode) return 'Default';
  return options?.modes.find((m) => m.id === mode)?.label ?? mode;
}

/** What is chosen for a turn, in a few words: `Plan · Opus · high`. */
export function optionsLine(options: AgentOptions | undefined, chosen: TurnOptions): string {
  const parts = [modeLabel(options, chosen.mode)];
  if (chosen.model) parts.push(options?.models.find((m) => m.id === chosen.model)?.label ?? chosen.model);
  if (chosen.effort) parts.push(chosen.effort);
  return parts.join(' · ');
}

/**
 * `chosen` with whatever the agent over there does not offer taken out: a
 * mode or model it no longer has, an effort the model cannot be asked for.
 * Nothing is taken out before the agent said what it offers.
 */
export function fitOptions(options: AgentOptions | undefined, chosen: TurnOptions): TurnOptions {
  if (!options) return chosen;
  const mode = options.modes.some((m) => m.id === chosen.mode) ? chosen.mode : undefined;
  const known = options.models.find((m) => m.id === chosen.model);
  // An agent that lists no models takes whatever name it is given.
  const model = options.models.length === 0 || known ? chosen.model : undefined;
  const efforts = effortsFor(options, model);
  const effort = chosen.effort && efforts.includes(chosen.effort) ? chosen.effort : undefined;
  return { mode, model, effort };
}

/**
 * The modes this app made up when every agent ran one prompt at a time. No
 * agent has them: a chat or a preference that still names one is left to the
 * computer's own default, and never shown by its raw name.
 */
const LEGACY_MODES = new Set(['default', 'edit', 'accept-edits', 'bypass']);

/** `chosen` without a mode from before the agents were driven as sessions. */
export function withoutLegacyMode(chosen: TurnOptions | undefined): TurnOptions {
  if (!chosen) return {};
  return chosen.mode && LEGACY_MODES.has(chosen.mode) ? { ...chosen, mode: undefined } : chosen;
}

/**
 * What is stored for the chats and the preferences of one agent of one
 * computer, fitted to what that agent says it offers: the entries that
 * changed, by key. Nothing changes while the agent has not said.
 */
export function refitStored<K extends string>(
  options: AgentOptions | undefined,
  stored: Record<K, TurnOptions | undefined>,
): Partial<Record<K, TurnOptions>> {
  const changed: Partial<Record<K, TurnOptions>> = {};
  for (const key of Object.keys(stored) as K[]) {
    const before = stored[key];
    if (!before) continue;
    const after = fitOptions(options, withoutLegacyMode(before));
    if (after.mode !== before.mode || after.model !== before.model || after.effort !== before.effort) {
      changed[key] = after;
    }
  }
  return changed;
}

/** The efforts that can be asked for with `model` (its default when none is chosen). */
export function effortsFor(options: AgentOptions | undefined, model: string | undefined): string[] {
  if (!options) return [];
  const picked = options.models.find((m) => m.id === model) ?? options.models[0];
  return picked?.efforts ?? [];
}

/** Whether `mode` lets the agent do anything without asking. */
export function isDangerous(options: AgentOptions | undefined, mode: string | undefined): boolean {
  if (!mode) return false;
  const known = options?.modes.find((m) => m.id === mode);
  return known ? known.dangerous : /bypass/i.test(mode);
}

/**
 * `text` with the paths inside the folder `cwd` said from it, as a person
 * reads them on a phone. A folder that only begins like `cwd` (`leon-web`
 * beside `leon`) is another folder and is left whole.
 */
export function fromCwd(text: string, cwd: string | undefined): string {
  const root = cwd?.replace(/[\\/]+$/, '') ?? '';
  if (root.length < 2) return text;
  let out = '';
  let from = 0;
  for (;;) {
    const at = text.indexOf(root, from);
    if (at < 0) break;
    const before = at === 0 ? '' : text[at - 1]!;
    const after = text[at + root.length] ?? '';
    // A path starts here only when nothing of a longer path precedes it.
    const starts = !/[\w.\-/\\]/.test(before);
    out += text.slice(from, at);
    if (starts && (after === '/' || after === '\\')) {
      from = at + root.length + 1;
    } else if (starts && !/[\w.\-]/.test(after)) {
      out += '.';
      from = at + root.length;
    } else {
      out += root;
      from = at + root.length;
    }
  }
  return out + text.slice(from);
}
