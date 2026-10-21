// The live side of the application: connections to the paired computers, the
// turns in flight and the messages of the open chats. Screens read this store
// and call the functions below; they never reach the link service themselves.
import { AppState } from 'react-native';
import { create } from 'zustand';

import type { ChatRecord, Worktrees } from '@/chat/list';
import { questionLine, refitStored, withoutLegacyMode, type Message } from '@/chat/model';
import {
  initialTurn,
  isOver,
  isWaiting,
  markAnswered,
  pendingQuestions,
  reduceTurn,
  sayMore,
  type TurnState,
} from '@/chat/reduce';
import {
  type Activity,
  combineRows,
  mergeTranscript,
  nextActivity,
  refreshRows,
  transcriptRows,
} from '@/chat/transcript';
import { tokensLine } from '@/chat/tokens';
import { createDemoLink, DEMO_HOST, isDemo } from '@/link/demo';
import { explain } from '@/link/errors';
import {
  LinkError,
  type AgentOptions,
  type Answer,
  type ConnState,
  type HostLink,
  type LinkService,
  type DenCub,
  type SharedState,
  type TurnEvent,
  type TurnHandle,
  type TurnOptions,
} from '@/link/types';

import { announce } from './alerts';
import { optionsKey, useApp } from './app';
import { forgetLogos } from './logos';
import { readJson, remove, writeJson } from './storage';

type RuntimeState = {
  conn: Record<string, ConnState>;
  shared: Record<string, SharedState>;
  /** Who is in each computer's Den, as it last said; absent for one without a Den. */
  dens: Record<string, DenCub[]>;
  /** Installed agents per computer (catalogue tags). */
  installed: Record<string, string[]>;
  /** The git worktrees of each computer's shared projects, by project root. */
  worktrees: Record<string, Worktrees>;
  /** Messages this device holds, per chat. */
  messages: Record<string, Message[]>;
  /** The shared transcript of a chat that began on the other computer. */
  transcripts: Record<string, Message[]>;
  turns: Record<string, TurnState>;
  /** The tokens of each chat's last turn that ended here, in a few words (`tokensLine`). */
  lastTokens: Record<string, string>;
  /** How each shared session is moving on its computer, per chat. */
  activity: Record<string, Activity>;
  /** What each agent offers on each computer (`optionsKey`), once it was asked. */
  options: Record<string, AgentOptions>;
};

export const useRuntime = create<RuntimeState>(() => ({
  conn: {},
  shared: {},
  dens: {},
  installed: {},
  worktrees: {},
  messages: {},
  transcripts: {},
  turns: {},
  lastTokens: {},
  activity: {},
  options: {},
}));

export { combineRows };

const messagesKey = (chatId: string) => `leon.messages.v1.${chatId}`;

// --- The service -----------------------------------------------------------

let service: LinkService | null = null;

/** Replaces the service: tests and previews pass a fake. */
export function setLinkService(next: LinkService | null): void {
  service = next;
}

export function link(): LinkService {
  if (!service) {
    // Required here, not imported at the top, so that nothing loads the
    // native module until the application actually needs it.
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const { createNativeLink } = require('@/link/native') as typeof import('@/link/native');
    service = createNativeLink(useApp.getState().deviceName);
  }
  return service;
}

// --- Connections -----------------------------------------------------------

const links = new Map<string, HostLink>();
const tokens = new Map<string, object>();

function setConn(hostId: string, state: ConnState): void {
  useRuntime.setState((s) => ({ conn: { ...s.conn, [hostId]: state } }));
}

/** The connection to a paired computer, opened the first time it is asked for. */
export function ensureLink(hostId: string): HostLink | undefined {
  const existing = links.get(hostId);
  if (existing) return existing;
  const host = useApp.getState().hosts.find((h) => h.hostId === hostId);
  if (!host) return undefined;
  try {
    // The listener outlives a forgotten computer: the token tells it so.
    const token = {};
    tokens.set(hostId, token);
    const onState = (state: ConnState) => {
      if (tokens.get(hostId) !== token) return;
      setConn(hostId, state);
      if (state.kind === 'online') void whenOnline(hostId);
    };
    const opened = isDemo(hostId) ? createDemoLink(onState) : link().connect(host, onState);
    links.set(hostId, opened);
    return opened;
  } catch (error) {
    setConn(hostId, {
      kind: 'offline',
      reason: 'other',
      message: explain(error, host.relayUrl).headline,
      attempt: 0,
    });
    return undefined;
  }
}

/** Adds the demo computer, which needs no Leon to connect to; returns its id. */
export function addDemo(): string {
  useApp.getState().addHost(DEMO_HOST);
  ensureLink(DEMO_HOST.hostId);
  return DEMO_HOST.hostId;
}

/** Opens every connection: the list of computers shows who is reachable. */
export function connectAll(): void {
  for (const host of useApp.getState().hosts) ensureLink(host.hostId);
}

/** Forgets a computer: its connection, its chats and their messages. */
export function forgetHost(hostId: string): void {
  tokens.delete(hostId);
  links.get(hostId)?.close();
  links.delete(hostId);
  const gone = useApp.getState().removeHost(hostId);
  forgetLogos(hostId);
  for (const chatId of gone) {
    handles.delete(chatId);
    remove(messagesKey(chatId));
  }
  useRuntime.setState((s) => {
    const drop = <T,>(map: Record<string, T>, keys: string[]) =>
      Object.fromEntries(Object.entries(map).filter(([k]) => !keys.includes(k)));
    return {
      conn: drop(s.conn, [hostId]),
      shared: drop(s.shared, [hostId]),
      dens: drop(s.dens, [hostId]),
      installed: drop(s.installed, [hostId]),
      worktrees: drop(s.worktrees, [hostId]),
      messages: drop(s.messages, gone),
      transcripts: drop(s.transcripts, gone),
      turns: drop(s.turns, gone),
      lastTokens: drop(s.lastTokens, gone),
      activity: drop(s.activity, gone),
      options: Object.fromEntries(
        Object.entries(s.options).filter(([key]) => !key.startsWith(`${hostId}/`)),
      ),
    };
  });
}

/**
 * A computer that was paired again under a new identity (its Leon was
 * reinstalled, or its files moved) leaves its old entry behind, which can
 * never connect again: the entries of the same name on the same relay that
 * are not reachable now.
 */
export function staleTwins(fresh: { hostId: string; hostName: string; relayUrl: string }): string[] {
  return useApp
    .getState()
    .hosts.filter(
      (h) =>
        h.hostId !== fresh.hostId &&
        h.hostName === fresh.hostName &&
        h.relayUrl === fresh.relayUrl &&
        useRuntime.getState().conn[h.hostId]?.kind !== 'online',
    )
    .map((h) => h.hostId);
}

/** Tries every connection that is down now: called when the app comes forward. */
export function wake(): void {
  for (const [hostId, opened] of links) {
    const state = useRuntime.getState().conn[hostId];
    if (state?.kind === 'offline') opened.reconnectNow();
    else if (state?.kind === 'online') void reattach(hostId);
  }
}

export function retry(hostId: string): void {
  const opened = ensureLink(hostId);
  opened?.reconnectNow();
}

async function whenOnline(hostId: string): Promise<void> {
  void refreshShared(hostId);
  void refreshInstalled(hostId);
  await reattach(hostId);
}

/**
 * Reads again what the other computer shares, and the git worktrees of its
 * projects, which say where the sessions of a project ran. Both arrive
 * together so the list is drawn once; failures leave what was known.
 */
export async function refreshShared(hostId: string, withTrees = true): Promise<void> {
  const opened = ensureLink(hostId);
  if (!opened) return;
  let shared: SharedState;
  try {
    shared = await opened.shareState();
  } catch {
    // Offline or busy: the connection banner already says so.
    return;
  }
  const roots = [...new Set(shared.projects.map((project) => project.root).filter(Boolean))];
  let trees: Worktrees | undefined;
  // Worktrees change far less often than sessions, and cost a command on the
  // other computer: a quick reading keeps the ones already known.
  if (roots.length > 0 && (withTrees || !useRuntime.getState().worktrees[hostId])) {
    try {
      const found = await opened.worktrees(roots);
      const listed: Worktrees = {};
      roots.forEach((root, at) => {
        listed[root.length > 1 ? root.replace(/[\\/]+$/, '') : root] = found[at] ?? [];
      });
      trees = listed;
    } catch {
      // The projects still list; their worktrees are asked for again next time.
    }
  }
  useRuntime.setState((s) => ({
    shared: { ...s.shared, [hostId]: shared },
    worktrees: trees ? { ...s.worktrees, [hostId]: trees } : s.worktrees,
  }));
}

const asking = new Set<string>();

/**
 * What the chats and the preference of one agent hold is fitted to what the
 * agent offers: a mode or a model it does not have (one from an older version
 * of this app, or of the agent) is forgotten, so the next turn runs with that
 * computer's own and nothing shows a name that means nothing.
 */
function refitChats(hostId: string, agent: string, offered: AgentOptions): void {
  const app = useApp.getState();
  const mine = Object.values(app.chats).filter((c) => c.hostId === hostId && c.agent === agent);
  const changed = refitStored(offered, Object.fromEntries(mine.map((c) => [c.id, c.options])));
  for (const [id, options] of Object.entries(changed)) {
    if (options) app.patchChat(id, { options });
  }
  const key = optionsKey(hostId, agent);
  const last = refitStored(offered, { [key]: app.lastOptions[key] })[key];
  if (last) app.rememberOptions(hostId, agent, last);
}

/**
 * Asks an agent on a computer what it offers (modes, models, efforts), once
 * per session of the app unless `again`. The answer lands in the store.
 */
export async function loadAgentOptions(hostId: string, agent: string, again = false): Promise<void> {
  const key = optionsKey(hostId, agent);
  if (asking.has(key) || (!again && useRuntime.getState().options[key])) return;
  const opened = ensureLink(hostId);
  if (!opened) return;
  asking.add(key);
  try {
    const options = await opened.agentOptions(agent);
    useRuntime.setState((s) => ({ options: { ...s.options, [key]: options } }));
    refitChats(hostId, agent, options);
  } catch {
    // Offline or busy: asked again the next time a chat needs them.
  } finally {
    asking.delete(key);
  }
}

async function refreshInstalled(hostId: string): Promise<void> {
  const opened = links.get(hostId);
  if (!opened) return;
  try {
    const installed = await opened.installedAgents();
    useRuntime.setState((s) => ({ installed: { ...s.installed, [hostId]: installed } }));
  } catch {
    // Asked again the next time the connection comes up.
  }
}

// --- Messages --------------------------------------------------------------

/** The messages this device holds of a chat, read from storage the first time. */
export function loadMessages(chatId: string): Message[] {
  const known = useRuntime.getState().messages[chatId];
  if (known) return known;
  const stored = readJson<Message[]>(messagesKey(chatId)) ?? [];
  useRuntime.setState((s) => ({ messages: { ...s.messages, [chatId]: stored } }));
  return stored;
}

function saveMessages(chatId: string, messages: Message[]): void {
  useRuntime.setState((s) => ({ messages: { ...s.messages, [chatId]: messages } }));
  writeJson(messagesKey(chatId), messages);
}

function appendMessages(chatId: string, more: Message[]): void {
  if (more.length === 0) return;
  saveMessages(chatId, [...loadMessages(chatId), ...more]);
}

/** Reads the shared transcript of a chat that continues a shared session. */
export async function loadTranscript(chat: ChatRecord): Promise<void> {
  if (!chat.externalId) return;
  const opened = ensureLink(chat.hostId);
  if (!opened) return;
  try {
    const entries = await opened.transcript(chat.agent, chat.externalId);
    if (!entries) return;
    const rows = refreshRows(
      useRuntime.getState().transcripts[chat.id],
      transcriptRows(chat.id, entries),
    );
    if (rows !== useRuntime.getState().transcripts[chat.id]) {
      useRuntime.setState((s) => ({ transcripts: { ...s.transcripts, [chat.id]: rows } }));
    }
    // Drop this device's copy of the turns the transcript now has, unless a
    // turn is in flight (its rows are appended when it ends).
    if (!useRuntime.getState().turns[chat.id]) {
      const local = loadMessages(chat.id);
      const kept = mergeTranscript(rows, local).slice(rows.length);
      if (kept.length !== local.length) saveMessages(chat.id, kept);
    }
  } catch {
    // Offline: what this device holds is shown meanwhile.
  }
}

// --- Following the other computer --------------------------------------------
//
// Nothing is pushed to this device: what the other computer shares is read
// again while a screen that shows it is in front. Its own Leon imports an
// agent's writing into the history a moment after the agent goes quiet (and
// once a minute whatever happens), so that is how far behind this can be.

/** How often an open chat asks whether its session moved. */
const CHAT_EVERY_MS = 4_000;
/** How often an open list of sessions is read again. */
const LIST_EVERY_MS = 20_000;
/** Every this many readings of a list, its worktrees are asked for again too. */
const TREES_EVERY = 6;

function inFront(hostId: string): boolean {
  return AppState.currentState === 'active' && useRuntime.getState().conn[hostId]?.kind === 'online';
}

/** Runs `tick` now and every `every` ms, never two at once, until the returned function is called. */
function every(ms: number, tick: () => Promise<void>): () => void {
  let busy = false;
  let over = false;
  const run = () => {
    if (busy || over) return;
    busy = true;
    void tick().finally(() => {
      busy = false;
    });
  };
  run();
  const timer = setInterval(run, ms);
  // Coming back to the front should not wait for the next tick.
  const woke = AppState.addEventListener('change', (state) => {
    if (state === 'active') run();
  });
  return () => {
    over = true;
    clearInterval(timer);
    woke.remove();
  };
}

/**
 * Follows the session of a chat while its screen is in front: asks what the
 * other computer shares, and reads the transcript again only when the session
 * moved. A turn this device is running is left alone; its rows are its own
 * until it ends. Returns what stops it.
 */
export function watchChat(chatId: string): () => void {
  return every(CHAT_EVERY_MS, async () => {
    const chat = useApp.getState().chats[chatId];
    if (!chat?.externalId || !inFront(chat.hostId)) return;
    if (useRuntime.getState().turns[chatId] || chat.activeTurn) return;
    const opened = links.get(chat.hostId);
    if (!opened) return;
    let shared: SharedState;
    try {
      shared = await opened.shareState();
    } catch {
      return;
    }
    useRuntime.setState((s) => ({ shared: { ...s.shared, [chat.hostId]: shared } }));
    const session = shared.sessions.find(
      (s) => s.agent === chat.agent && s.externalId === chat.externalId,
    );
    if (!session) return;
    const { activity, changed } = nextActivity(
      useRuntime.getState().activity[chatId],
      session,
      Date.now(),
    );
    useRuntime.setState((s) => ({ activity: { ...s.activity, [chatId]: activity } }));
    if (changed || !useRuntime.getState().transcripts[chatId]) await loadTranscript(chat);
  });
}

/** Reads a computer's sessions again while one of its lists is in front. */
/** How often the Den in front asks each computer who is in it. */
const DEN_EVERY_MS = 3_000;

/**
 * Follows the Den of every computer while its screen is in front. Asking is
 * what keeps a computer working its Den out, so it stops when the screen is
 * left. A computer that has no Den (an older Leon, a headless host) is not
 * asked: it would drop the connection.
 */
export function watchDens(): () => void {
  return every(DEN_EVERY_MS, askDens);
}

/**
 * Asks every computer that has a Den who is in it, once. The home asks when
 * it comes to the front, so that its card says who waits; a computer works
 * its Den out for a few seconds after being asked, and then stops.
 */
export async function askDens(): Promise<void> {
  const hosts = useApp.getState().hosts;
  await Promise.all(
    hosts.map(async ({ hostId }) => {
      if (!inFront(hostId)) return;
      const opened = ensureLink(hostId);
      if (!opened) return;
      if (!useRuntime.getState().shared[hostId]) await refreshShared(hostId, false);
      if (!useRuntime.getState().shared[hostId]?.den) return;
      try {
        const cubs = await opened.den();
        useRuntime.setState((s) => ({ dens: { ...s.dens, [hostId]: cubs } }));
      } catch {
        // Offline or busy: what was known stays until it answers again.
      }
    }),
  );
}

export function watchHost(hostId: string): () => void {
  let readings = 0;
  return every(LIST_EVERY_MS, async () => {
    if (!inFront(hostId)) return;
    const withTrees = readings % TREES_EVERY === 0;
    readings += 1;
    await refreshShared(hostId, withTrees);
  });
}

// --- Turns -----------------------------------------------------------------

const handles = new Map<string, TurnHandle>();
/** Chats whose turn was asked for and has not started yet. */
const starting = new Set<string>();
/** Events waiting for the next frame, per chat, with the turn they belong to. */
const pending = new Map<string, { turnId: string; events: TurnEvent[] }>();
let scheduled = false;

function setTurn(chatId: string, turn: TurnState | undefined): void {
  useRuntime.setState((s) => {
    if (turn) return { turns: { ...s.turns, [chatId]: turn } };
    const { [chatId]: _gone, ...rest } = s.turns;
    return { turns: rest };
  });
}

/** Text arrives a few characters at a time: apply a frame's worth at once. */
function enqueue(chatId: string, turnId: string, event: TurnEvent): void {
  const queue = pending.get(chatId);
  if (queue && queue.turnId === turnId) queue.events.push(event);
  else pending.set(chatId, { turnId, events: [event] });
  if (scheduled) return;
  scheduled = true;
  // Behind another application no frame is drawn and no timer fires, and
  // what the agent says must still be heard there: a question is announced.
  if (AppState.currentState === 'active' && globalThis.requestAnimationFrame) {
    globalThis.requestAnimationFrame(flush);
  } else {
    queueMicrotask(flush);
  }
}

function flush(): void {
  scheduled = false;
  const batches = [...pending.entries()];
  pending.clear();
  for (const [chatId, { turnId, events }] of batches) apply(chatId, turnId, events);
}

/**
 * A turn followed again is replayed from its start, in a burst: what it says
 * then already happened, and is not announced. Per chat, until when.
 */
const quiet = new Map<string, number>();
const REPLAY_MS = 2_500;

/** Tells the person what a batch of events changed for them, when it is news. */
function tellPerson(chatId: string, before: TurnState, after: TurnState, events: TurnEvent[]): void {
  const now = Date.now();
  if ((quiet.get(chatId) ?? 0) > now) {
    // Still arriving in a burst: it is the replay.
    quiet.set(chatId, now + 400);
    return;
  }
  const chat = useApp.getState().chats[chatId];
  if (!chat) return;
  const agentName = link().agents().find((a) => a.id === chat.agent)?.name ?? chat.agent;
  if (!isWaiting(before) && isWaiting(after)) {
    const first = pendingQuestions(after).find((q) => q.status === 'open');
    announce(chatId, 'waiting', agentName, first ? questionLine(first.ask) : '');
    return;
  }
  const finished = events.find((e) => e.type === 'finished');
  if (before.status === 'running' && finished?.type === 'finished' && !finished.interrupted) {
    const said = [...after.items].reverse().find((m) => m.kind === 'assistant');
    // A notification shows the marks of markdown as they are: without them.
    const first = said?.kind === 'assistant' ? (said.text.trim().split('\n')[0] ?? '') : '';
    const line = first.replace(/^#+\s*/, '').replace(/[*`]/g, '').slice(0, 120);
    announce(chatId, finished.ok ? 'done' : 'failed', agentName, line);
  }
}

function apply(chatId: string, turnId: string, events: TurnEvent[]): void {
  let turn = useRuntime.getState().turns[chatId];
  // A listener of a turn that was replaced or already ended: nothing to do.
  if (!turn || turn.turnId !== turnId) return;
  const now = Date.now();
  const before = turn.sessionId;
  const modeBefore = turn.mode;
  const was = turn;
  for (const event of events) turn = reduceTurn(turn, event, now);
  tellPerson(chatId, was, turn, events);

  const chatNow = useApp.getState().chats[chatId];
  // The agent changed its mode while working (a plan was approved, "always
  // allow" switched it): the chat goes on in that mode, if it is one that can
  // be asked for.
  if (chatNow && modeBefore !== undefined && turn.mode !== modeBefore) {
    const offered = useRuntime.getState().options[optionsKey(chatNow.hostId, chatNow.agent)];
    const mode = offered?.modes.some((m) => m.id === turn!.mode) ? turn.mode : undefined;
    useApp.getState().patchChat(chatId, { options: { ...chatNow.options, mode } });
  }
  const waiting = isWaiting(turn);
  if (chatNow?.activeTurn && (chatNow.activeTurn.waiting === true) !== waiting) {
    useApp.getState().patchChat(chatId, { activeTurn: { ...chatNow.activeTurn, waiting } });
  }

  if (turn.sessionId && turn.sessionId !== before) {
    const chat = useApp.getState().chats[chatId];
    if (chat) {
      useApp.getState().patchChat(chatId, {
        resumeId: turn.sessionId,
        sessionIds: chat.sessionIds.includes(turn.sessionId)
          ? chat.sessionIds
          : [...chat.sessionIds, turn.sessionId],
      });
    }
  }

  if (isOver(turn)) {
    const line = tokensLine(turn.tokens, turn.durationMs);
    useRuntime.setState((st) => {
      const { [chatId]: _was, ...rest } = st.lastTokens;
      return { lastTokens: line ? { ...rest, [chatId]: line } : rest };
    });
    endTurn(chatId, turn.items);
  } else {
    setTurn(chatId, turn);
  }
}

/** Keeps the rows of a turn that ended and clears everything live about it. */
function endTurn(chatId: string, rows: Message[]): void {
  handles.delete(chatId);
  appendMessages(chatId, rows);
  setTurn(chatId, undefined);
  useApp.getState().patchChat(chatId, { activeTurn: undefined, updatedAt: Date.now() });
}

function failTurn(chatId: string, turnId: string, text: string): void {
  const turn = useRuntime.getState().turns[chatId];
  if (turn && turn.turnId !== turnId) return;
  endTurn(chatId, [
    ...(turn?.items ?? []),
    { id: `${turnId}:x`, kind: 'notice', tone: 'error', text, at: Date.now() },
  ]);
}

function titleFrom(prompt: string): string {
  const line = prompt.trim().split('\n')[0] ?? '';
  return line.length > 60 ? `${line.slice(0, 59)}…` : line;
}

/**
 * Sends a prompt in a chat. The answer arrives in the store. While the agent
 * of the chat is working, and can be told more, the text is added to what it
 * has to answer instead of starting anything.
 */
export async function send(chatId: string, prompt: string, asked: TurnOptions): Promise<void> {
  // A mode from before the agents were driven as sessions is nobody's.
  const options = withoutLegacyMode(asked);
  const text = prompt.trim();
  const chat = useApp.getState().chats[chatId];
  if (!chat || !text) return;
  const running = useRuntime.getState().turns[chatId];
  if (running) {
    const handle = handles.get(chatId);
    if (!handle) return;
    try {
      handle.say(text);
      const turn = useRuntime.getState().turns[chatId];
      if (turn) setTurn(chatId, sayMore(turn, text, Date.now()));
    } catch (error) {
      tell(chatId, error);
    }
    return;
  }

  const now = Date.now();
  const turnId = `${chatId}#${now.toString(36)}`;
  appendMessages(chatId, [{ id: `${turnId}:u`, kind: 'user', text, at: now }]);
  setTurn(chatId, initialTurn(turnId));
  useApp.getState().patchChat(chatId, {
    title: chat.title || titleFrom(text),
    options,
    updatedAt: now,
    activeTurn: { turnId },
  });
  useApp.getState().rememberOptions(chat.hostId, chat.agent, options);

  const opened = ensureLink(chat.hostId);
  if (!opened) {
    failTurn(chatId, turnId, 'That computer is no longer paired with this device.');
    return;
  }
  starting.add(chatId);
  try {
    const handle = await opened.startTurn(
      {
        agent: chat.agent,
        cwd: chat.cwd,
        prompt: text,
        // A session of the history is continued by its own id, even when this
        // device first opened it before it could drive its agent.
        resumeId: chat.resumeId ?? chat.externalId,
        options,
        chatId,
      },
      (event) => enqueue(chatId, turnId, event),
    );
    // The turn may already be over (a command that was not found ends at once).
    if (useRuntime.getState().turns[chatId]?.turnId === turnId) {
      handles.set(chatId, handle);
      const current = useApp.getState().chats[chatId]?.activeTurn;
      useApp.getState().patchChat(chatId, { activeTurn: { ...current, turnId, pty: handle.pty } });
    }
  } catch (error) {
    const said = explain(error);
    failTurn(chatId, turnId, `${said.headline} ${said.advice}`);
  } finally {
    starting.delete(chatId);
  }
}

/** Says in the chat that something asked of the running turn was refused. */
function tell(chatId: string, error: unknown): void {
  const turn = useRuntime.getState().turns[chatId];
  if (!turn) return;
  const said = explain(error);
  setTurn(chatId, {
    ...turn,
    items: [
      ...turn.items,
      {
        id: `${turn.turnId}:n${turn.made}`,
        kind: 'notice',
        tone: 'warning',
        text: `${said.headline} ${said.advice}`.trim(),
        at: Date.now(),
      },
    ],
    made: turn.made + 1,
  });
}

/**
 * Answers what the agent of a chat asked. The answer is kept until the agent
 * has it, across a dropped connection; the question shows it as on its way
 * until then.
 */
export function answer(chatId: string, questionId: string, reply: Answer): void {
  const handle = handles.get(chatId);
  const turn = useRuntime.getState().turns[chatId];
  if (!handle || !turn) return;
  try {
    handle.answer(questionId, reply);
    setTurn(chatId, markAnswered(turn, questionId, reply));
    const chat = useApp.getState().chats[chatId];
    if (chat?.activeTurn) {
      useApp.getState().patchChat(chatId, {
        activeTurn: { ...chat.activeTurn, waiting: isWaiting(useRuntime.getState().turns[chatId]) },
      });
    }
  } catch (error) {
    tell(chatId, error);
  }
}

/**
 * Asks the agent of a chat to stop: it ends the turn itself and keeps the
 * conversation. An agent that cannot be asked is killed instead.
 */
export function stop(chatId: string): void {
  const handle = handles.get(chatId);
  if (!handle) return;
  try {
    handle.interrupt();
  } catch (error) {
    if (error instanceof LinkError && error.code === 'NotSupported') handle.cancel();
    else tell(chatId, error);
  }
}

/** Kills the agent of a chat, whatever it was doing. */
export function kill(chatId: string): void {
  handles.get(chatId)?.cancel();
}

/**
 * Changes how the turns of a chat are run. A turn in flight is told the new
 * mode and model at once, when its agent can be; the effort applies from the
 * next turn.
 */
export function setOptions(chatId: string, next: TurnOptions): void {
  const chat = useApp.getState().chats[chatId];
  if (!chat) return;
  const before = chat.options ?? {};
  useApp.getState().patchChat(chatId, { options: next });
  useApp.getState().rememberOptions(chat.hostId, chat.agent, next);
  const handle = handles.get(chatId);
  if (!handle || !useRuntime.getState().turns[chatId]) return;
  const offered = useRuntime.getState().options[optionsKey(chat.hostId, chat.agent)];
  if (!offered?.liveChanges) return;
  try {
    if (next.mode && next.mode !== before.mode) handle.setMode(next.mode);
    if (next.model && next.model !== before.model) handle.setModel(next.model);
  } catch (error) {
    tell(chatId, error);
  }
}

const attaching = new Set<string>();

/**
 * Follows again the turns that were running when the app was closed. The
 * other computer replays each from its start, so its rows are rebuilt.
 */
export async function reattach(hostId: string): Promise<void> {
  const opened = links.get(hostId);
  if (!opened) return;
  const orphans = Object.values(useApp.getState().chats).filter(
    (c) =>
      c.hostId === hostId &&
      c.activeTurn &&
      !handles.has(c.id) &&
      !starting.has(c.id) &&
      !attaching.has(c.id),
  );
  if (orphans.length === 0) return;
  for (const chat of orphans) attaching.add(chat.id);
  try {
    const running = await opened.runningTurns();
    for (const chat of orphans) {
      const { turnId } = chat.activeTurn!;
      const pty =
        chat.activeTurn!.pty ??
        running
          .filter((r) => r.chatId === chat.id)
          .sort((a, b) => b.startedUnix - a.startedUnix)[0]?.pty;
      loadMessages(chat.id);
      if (!pty || !running.some((r) => r.pty === pty)) {
        setTurn(chat.id, initialTurn(turnId));
        failTurn(
          chat.id,
          turnId,
          'This answer ended while the app was closed, and the other computer no longer keeps it. Ask again, or look for it in the history of that computer.',
        );
        continue;
      }
      setTurn(chat.id, initialTurn(turnId));
      quiet.set(chat.id, Date.now() + REPLAY_MS);
      try {
        const handle = await opened.attachTurn(pty, (event) => enqueue(chat.id, turnId, event));
        if (useRuntime.getState().turns[chat.id]?.turnId === turnId) handles.set(chat.id, handle);
      } catch (error) {
        const said = explain(error);
        failTurn(chat.id, turnId, `${said.headline} ${said.advice}`);
      }
    }
  } catch {
    // Offline again: tried the next time the connection comes up.
  } finally {
    for (const chat of orphans) attaching.delete(chat.id);
  }
}

/** Removes a chat this device holds. A shared session stays in the history. */
export function forgetChat(chatId: string): void {
  handles.delete(chatId);
  remove(messagesKey(chatId));
  useApp.getState().removeChat(chatId);
  useRuntime.setState((s) => {
    const { [chatId]: _m, ...messages } = s.messages;
    const { [chatId]: _t, ...transcripts } = s.transcripts;
    const { [chatId]: _u, ...turns } = s.turns;
    const { [chatId]: _l, ...lastTokens } = s.lastTokens;
    const { [chatId]: _a, ...activity } = s.activity;
    return { messages, transcripts, turns, lastTokens, activity };
  });
}
