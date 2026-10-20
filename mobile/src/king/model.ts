// King on a phone: what its service answers, read into the few things the
// screen shows. Pure: JSON in, plain values out. What the service sends is
// read defensively, since a computer may run an older or newer Leon: a field
// not known here is ignored, and one that is missing or of the wrong kind
// leaves its default.

/** What kind of thing an entry of a conversation is, as King's service says it. */
export type EntryKind = 'prompt' | 'news' | 'agent' | 'tool' | 'question' | 'notice' | 'report';

/** One thing a conversation said. */
export type KingEntry = { seq: number; atMs: number; kind: EntryKind; text: string };

/** A conversation, in short: King's own or a worker's. */
export type KingConversation = {
  agent: string;
  model?: string;
  effort?: string;
  /** In a turn. */
  busy: boolean;
  busySinceMs?: number;
  /** How many questions it waits on. */
  questions: number;
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
  state: WorkerState;
  conversation: KingConversation;
  /** The last thing it answered, cut short. */
  lastAnswer?: string;
  updatedMs: number;
};

/** A task of the board carried out as its steps. */
export type KingRun = {
  id: number;
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

function conversationOf(value: unknown): KingConversation {
  const json = record(value) ?? {};
  return {
    agent: text(json.agent) ?? '',
    model: text(json.model) || undefined,
    effort: text(json.effort) || undefined,
    busy: json.busy === true,
    busySinceMs: count(json.busy_since_ms),
    questions: list(json.questions).length,
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
