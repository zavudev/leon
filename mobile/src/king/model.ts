// King on a phone: what its service answers, read into the few things the
// screen shows. Pure: JSON in, plain values out. What the service sends is
// read defensively, since a computer may run an older or newer Leon: a field
// not known here is ignored, and one that is missing or of the wrong kind
// leaves its default.

import type { KingRequest, KingTarget } from '@/link/types';

/** What kind of thing an entry of a conversation is, as King's service says it. */
export type EntryKind = 'prompt' | 'news' | 'agent' | 'tool' | 'question' | 'notice' | 'report';

/** One thing a conversation said. */
export type KingEntry = { seq: number; atMs: number; kind: EntryKind; text: string };

/**
 * How a question may be answered: one of a permission's choices, by id, each
 * with what it does (`allow`, `always`, `deny`); an option, by label, or
 * words of one's own; a plan approved or not; or, for a form this app does
 * not know, words alone.
 */
export type QuestionForm =
  | { kind: 'permission'; choices: { id: string; effect: string }[] }
  /** `options`: the labels, read from the question's text; none when they could not be. */
  | { kind: 'options'; options: string[] }
  | { kind: 'plan' }
  | { kind: 'words' };

/** A question King or a worker asked and nobody answered yet. */
export type KingQuestion = { id: string; text: string; form: QuestionForm };

/** A conversation, in short: King's own or a worker's. */
export type KingConversation = {
  agent: string;
  model?: string;
  effort?: string;
  /** In a turn. */
  busy: boolean;
  busySinceMs?: number;
  /** The questions it waits on, oldest first. */
  questions: KingQuestion[];
  costUsd: number;
  /** The number of its newest entry. */
  lastSeq?: number;
};

/**
 * Where a worker is: `working`, `asking`, `done`, `failed`, `paused`,
 * `stopped` or `released`. One not listed is some kind of working.
 */
export type WorkerState = string;

export type KingWorker = {
  id: number;
  title: string;
  project?: string;
  cwd: string;
  /** The machine it works on, by Leon's id of it; this computer when absent. */
  machine?: string;
  /** It runs in a terminal of the desktop window: it cannot be told more from here. */
  terminal: boolean;
  state: WorkerState;
  conversation: KingConversation;
  /** The last thing it answered, cut short. */
  lastAnswer?: string;
  updatedMs: number;
};

/** A task of the board carried out as its steps. */
export type KingRun = {
  id: number;
  /** Its task, by id. */
  task: string;
  title: string;
  project: string;
  /** `running`, `blocked`, `done` or `canceled`. */
  state: string;
  /** The state of each of its steps. */
  steps: string[];
  /** Why it is blocked, while it is. */
  note?: string;
};

/** King and its workers, in short. */
export type KingStatus = {
  /** The number of King's conversation: another one once a new one is started. */
  conversation: number;
  brain: KingConversation;
  /** Newest last, as the service lists them. */
  workers: KingWorker[];
  /** What King has not been told yet. */
  news: number;
  spentTodayUsd: number;
  dailyBudgetUsd?: number;
  runs: KingRun[];
};

/** What King's service says without being asked. */
export type KingNotice =
  | { kind: 'changed'; status: KingStatus }
  /** `echo`: it is in King's conversation already. */
  | { kind: 'notify'; title: string; body: string; echo: boolean };

/** How many entries a transcript is asked for at once, the newest. */
export const TRANSCRIPT_LIMIT = 200;

/** How many entries of a conversation the phone keeps, as many as the service does. */
export const KEPT = 400;

type Json = Record<string, unknown>;

function record(value: unknown): Json | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? (value as Json) : undefined;
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function count(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined;
}

function list(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

const KINDS = new Set<string>(['prompt', 'news', 'agent', 'tool', 'question', 'notice', 'report']);

/**
 * The labels of the options a question offers. The service gives them only in
 * its text, `<question> Options: a | b | c.`, and only one question's are
 * offered: an option picked answers every question of the form.
 */
export function optionLabels(said: string): string[] {
  const at = said.indexOf('Options: ');
  if (at < 0 || said.indexOf('Options: ', at + 1) >= 0) return [];
  return said
    .slice(at + 'Options: '.length)
    .trim()
    .replace(/\.$/, '')
    .split(' | ')
    .map((label) => label.trim())
    .filter(Boolean);
}

function formOf(value: unknown, said: string): QuestionForm {
  const json = record(value);
  switch (json?.type) {
    case 'permission': {
      const choices = list(json.choices)
        .map((pair) => {
          const [id, effect] = Array.isArray(pair) ? pair : [];
          return typeof id === 'string' && typeof effect === 'string' ? { id, effect } : undefined;
        })
        .filter(defined);
      return choices.length > 0 ? { kind: 'permission', choices } : { kind: 'words' };
    }
    case 'choices':
      return { kind: 'options', options: list(json.questions).length === 1 ? optionLabels(said) : [] };
    case 'plan':
      return { kind: 'plan' };
    default:
      return { kind: 'words' };
  }
}

function questionOf(value: unknown): KingQuestion | undefined {
  const json = record(value);
  const id = text(json?.id);
  if (!json || !id) return undefined;
  const said = text(json.text) ?? '';
  return { id, text: said, form: formOf(json.form, said) };
}

function conversationOf(value: unknown): KingConversation {
  const json = record(value) ?? {};
  return {
    agent: text(json.agent) ?? '',
    model: text(json.model) || undefined,
    effort: text(json.effort) || undefined,
    busy: json.busy === true,
    busySinceMs: count(json.busy_since_ms),
    questions: list(json.questions).map(questionOf).filter(defined),
    costUsd: count(json.cost_usd) ?? 0,
    lastSeq: count(json.last_seq),
  };
}

function workerOf(value: unknown): KingWorker | undefined {
  const json = record(value);
  const id = count(json?.id);
  if (!json || id === undefined) return undefined;
  return {
    id,
    title: text(json.title) || `Worker #${id}`,
    project: text(json.project) || undefined,
    cwd: text(json.cwd) ?? '',
    machine: text(json.machine) || undefined,
    terminal: json.terminal === true,
    state: text(json.state) ?? 'working',
    conversation: conversationOf(json.conversation),
    lastAnswer: text(json.last_answer)?.trim() || undefined,
    updatedMs: count(json.updated_ms) ?? 0,
  };
}

function runOf(value: unknown): KingRun | undefined {
  const json = record(value);
  const id = count(json?.id);
  if (!json || id === undefined) return undefined;
  return {
    id,
    task: text(json.task) ?? '',
    title: text(json.title) ?? '',
    project: text(json.project) ?? '',
    state: text(json.state) ?? 'running',
    steps: list(json.steps).map((step) => text(record(step)?.state) ?? 'waiting'),
    note: text(json.note) || undefined,
  };
}

function defined<T>(value: T | undefined): value is T {
  return value !== undefined;
}

/** King's status from the answer to `status`, or from a notice; nothing when it is none. */
export function parseStatus(value: unknown): KingStatus | undefined {
  const json = record(value);
  if (!json || !record(json.brain)) return undefined;
  return {
    conversation: count(json.conversation) ?? 0,
    brain: conversationOf(json.brain),
    workers: list(json.workers).map(workerOf).filter(defined),
    news: count(json.news) ?? 0,
    spentTodayUsd: count(json.spent_today_usd) ?? 0,
    dailyBudgetUsd: count(json.daily_budget_usd),
    runs: list(json.runs).map(runOf).filter(defined),
  };
}

/** The entries of an answer to `transcript`, oldest first; the ones that make no sense are left out. */
export function parseEntries(value: unknown): KingEntry[] {
  return sortedBySeq(
    list(value)
      .map((item): KingEntry | undefined => {
        const json = record(item);
        const seq = count(json?.seq);
        const kind = text(json?.kind);
        const said = text(json?.text);
        if (!json || seq === undefined || !kind || !KINDS.has(kind) || said === undefined) return undefined;
        return { seq, atMs: count(json.at_ms) ?? 0, kind: kind as EntryKind, text: said };
      })
      .filter(defined),
  );
}

/** A notice of King's service, or nothing for one this app does not know. */
export function parseNotice(value: unknown): KingNotice | undefined {
  const json = record(value);
  if (json?.notice === 'changed') {
    const status = parseStatus(json.status);
    return status ? { kind: 'changed', status } : undefined;
  }
  if (json?.notice === 'notify') {
    return {
      kind: 'notify',
      title: text(json.title) ?? '',
      body: text(json.body) ?? '',
      echo: json.echo === true,
    };
  }
  return undefined;
}

function sortedBySeq(entries: KingEntry[]): KingEntry[] {
  return [...entries].sort((a, b) => a.seq - b.seq);
}

/**
 * What is known of a conversation with what was just read of it: one entry
 * per number, oldest first, at most `KEPT` of the newest.
 */
export function mergeEntries(have: KingEntry[], more: KingEntry[]): KingEntry[] {
  if (more.length === 0) return have;
  const bySeq = new Map<number, KingEntry>();
  for (const entry of have) bySeq.set(entry.seq, entry);
  for (const entry of more) bySeq.set(entry.seq, entry);
  return sortedBySeq([...bySeq.values()]).slice(-KEPT);
}

/** The number to ask for what came after: that of the newest entry known. */
export function lastSeq(entries: KingEntry[]): number | undefined {
  return entries.at(-1)?.seq;
}

/** What the conversation shows: an entry, or the tools used one after another, as one line. */
export type Piece =
  | { kind: 'one'; key: string; entry: KingEntry }
  | { kind: 'tools'; key: string; entries: KingEntry[] };

/** The entries as they are drawn: a run of tools is one piece, as on the desktop. */
export function piecesOf(entries: KingEntry[]): Piece[] {
  const pieces: Piece[] = [];
  for (const entry of entries) {
    const last = pieces.at(-1);
    if (entry.kind === 'tool') {
      if (last?.kind === 'tools') last.entries.push(entry);
      else pieces.push({ kind: 'tools', key: `tools-${entry.seq}`, entries: [entry] });
    } else {
      pieces.push({ kind: 'one', key: `entry-${entry.seq}`, entry });
    }
  }
  return pieces;
}

/** How a worker is doing, as far as the person has to care. */
export type WorkerMood = 'working' | 'asking' | 'done' | 'failed' | 'waiting' | 'off';

const WORKER_WORDS: Record<string, [WorkerMood, string]> = {
  working: ['working', 'Working'],
  asking: ['asking', 'Asks'],
  done: ['done', 'Done'],
  failed: ['failed', 'Failed'],
  paused: ['waiting', 'Stopped, waits'],
  stopped: ['off', 'Stopped'],
  released: ['off', 'Taken over'],
};

/** A worker's state in a word, and what it means. One not known here is some kind of working. */
export function workerWord(state: WorkerState): { mood: WorkerMood; word: string } {
  const [mood, word] = WORKER_WORDS[state] ?? ['working', 'Working'];
  return { mood, word };
}

/** The workers as the screen lists them: who needs the person, then who works, then the newest. */
export function workersInOrder(workers: KingWorker[]): KingWorker[] {
  const rank = (worker: KingWorker) => {
    const { mood } = workerWord(worker.state);
    return mood === 'asking' ? 0 : mood === 'working' ? 1 : 2;
  };
  return [...workers].sort((a, b) => rank(a) - rank(b) || b.updatedMs - a.updatedMs || b.id - a.id);
}

/** A run in one line: `Run #2 · Blocked · 3 of 5 done`. */
export function runLine(run: KingRun): string {
  const done = run.steps.filter((step) => step === 'done').length;
  const state = run.state ? run.state[0]!.toUpperCase() + run.state.slice(1) : 'Running';
  return `Run #${run.id} · ${state} · ${done} of ${run.steps.length} done`;
}

/** What a conversation runs on: `claude · opus · high`. */
export function runsOn(conversation: KingConversation): string {
  return [conversation.agent || 'agent', conversation.model, conversation.effort].filter(Boolean).join(' · ');
}

/** Dollars, to the cent: `$3.71`. */
export function dollars(usd: number): string {
  return `$${usd.toFixed(2)}`;
}

/** What was spent today, and of how much: `$3.71 today`, `$3.71 of $20.00 today`. */
export function spentLine(status: KingStatus): string {
  return status.dailyBudgetUsd !== undefined
    ? `${dollars(status.spentTodayUsd)} of ${dollars(status.dailyBudgetUsd)} today`
    : `${dollars(status.spentTodayUsd)} today`;
}

/** The first line of a text, for an entry shown folded. */
export function firstLine(said: string): string {
  return said.trim().split('\n')[0]?.trim() ?? '';
}

/** One way to answer a question at a press: what the button says, and the answer. */
export type Pick = { label: string; tone: 'go' | 'plain' | 'stop'; reply: KingReply };

/** An answer, as the service reads it beside the question. */
export type KingReply = { choice?: string; approve?: boolean; text?: string };

const EFFECTS: Record<string, [string, Pick['tone'], number]> = {
  allow: ['Allow', 'go', 0],
  always: ['Always', 'plain', 1],
  deny: ['Deny', 'stop', 2],
};

/**
 * The answers of a question one press gives, in the order they are drawn: a
 * permission's choices (allow, always, deny), an option per label, or
 * approve and reject for a plan. What is written in the field goes with it
 * (see `answerRequest`); some questions take words alone.
 */
export function picksOf(question: KingQuestion): Pick[] {
  const { form } = question;
  switch (form.kind) {
    case 'permission':
      return form.choices
        .map((choice) => {
          const [label, tone, rank] = EFFECTS[choice.effect] ?? [choice.id, 'plain', 3];
          return { rank, pick: { label, tone, reply: { choice: choice.id } } };
        })
        .sort((a, b) => a.rank - b.rank)
        .map(({ pick }) => pick);
    case 'options':
      return form.options.map((label) => ({ label, tone: 'plain', reply: { choice: label } }));
    case 'plan':
      return [
        { label: 'Approve', tone: 'go', reply: { approve: true } },
        { label: 'Reject', tone: 'stop', reply: { approve: false } },
      ];
    case 'words':
      return [];
  }
}

/** Whether a question can be answered with words alone: options, and the forms not known here. */
export function takesWords(question: KingQuestion): boolean {
  return question.form.kind === 'options' || question.form.kind === 'words';
}

/** What the field under a question is for. */
export function wordsHint(question: KingQuestion): string {
  switch (question.form.kind) {
    case 'permission':
      return 'Why not, or what to do instead (optional)';
    case 'plan':
      return 'What to change (optional)';
    case 'options':
      return question.form.options.length > 0 ? 'Or in your own words' : 'Your answer';
    case 'words':
      return 'Your answer';
  }
}

function trimmed(said: string | undefined): string | undefined {
  return said?.trim() || undefined;
}

/**
 * The request that answers question `question` of `target`'s conversation.
 * `words`, written in the field, go with the answer: a permission's reason,
 * an answer of one's own, what to change in a plan.
 */
export function answerRequest(target: KingTarget, question: string, reply: KingReply, words?: string): KingRequest {
  const request: Extract<KingRequest, { op: 'answer' }> = { op: 'answer', target, question };
  if (reply.choice !== undefined) request.choice = reply.choice;
  if (reply.approve !== undefined) request.approve = reply.approve;
  const said = trimmed(reply.text ?? words);
  if (said) request.text = said;
  return request;
}

/** What can be done to a worker from here, as it is now. */
export type WorkerActions = { tell: boolean; interrupt: boolean; stop: boolean };

/**
 * A worker taken over is no longer King's: nothing. One in a terminal of the
 * desktop window cannot be told more. Only a turn can be interrupted; one
 * stopped already is not stopped again (telling it more starts it again).
 */
export function workerActions(worker: KingWorker): WorkerActions {
  if (worker.state === 'released') return { tell: false, interrupt: false, stop: false };
  return {
    tell: !worker.terminal,
    interrupt: worker.conversation.busy,
    stop: worker.state !== 'stopped',
  };
}

export function tellWorkerRequest(worker: number, said: string): KingRequest {
  return { op: 'tell_worker', worker, text: said.trim() };
}

export function interruptRequest(target: KingTarget): KingRequest {
  return { op: 'interrupt', target };
}

export function stopWorkerRequest(worker: number): KingRequest {
  return { op: 'stop_worker', worker };
}

/** Where a worker works, when it is not this computer: `on build-box`. */
export function machineLine(worker: KingWorker): string | undefined {
  return worker.machine ? `on ${worker.machine}` : undefined;
}

/** What can be done to a run: go on with it while it is blocked, call it off until it is over. */
export type RunActions = { continue: boolean; cancel: boolean };

export function runActions(run: KingRun): RunActions {
  const over = run.state === 'done' || run.state === 'canceled';
  return { continue: run.state === 'blocked', cancel: !over };
}

/** A run of a task of the board, by its id or the end of it; nothing when none is written. */
export function runStartRequest(task: string): KingRequest | undefined {
  const id = task.trim().replace(/^#/, '');
  return id ? { op: 'run_start', task: id } : undefined;
}

/** Goes on with a blocked run; `said` is told to the workers of its failed steps. */
export function runContinueRequest(run: number, said?: string): KingRequest {
  const request: Extract<KingRequest, { op: 'run_continue' }> = { op: 'run_continue', run };
  const told = trimmed(said);
  if (told) request.text = told;
  return request;
}

export function runCancelRequest(run: number): KingRequest {
  return { op: 'run_cancel', run };
}
