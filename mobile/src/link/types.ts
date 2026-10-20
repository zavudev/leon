// What the rest of the application knows of the relay client: plain values,
// with no trace of the native bindings (no tagged classes, no bigint), so they
// can be stored, compared and built by hand in tests.

/**
 * An agent this app can drive. Each is spoken to both ways: it asks before it
 * acts and waits for the answer, and can be stopped while it works.
 */
export type AgentInfo = { id: string; name: string; program: string };

/** A permission mode of an agent, as installed on the other computer. */
export type AgentMode = { id: string; label: string; description?: string; dangerous: boolean };

/** A model of an agent; `efforts` are the ones it can be asked for, lightest first. */
export type AgentModel = { id: string; label: string; description?: string; efforts: string[] };

/** What an agent offers over there. `assumed`: this app's own list, the agent could not be asked. */
export type AgentOptions = {
  version?: string;
  modes: AgentMode[];
  defaultMode?: string;
  models: AgentModel[];
  questions: boolean;
  followUps: boolean;
  liveChanges: boolean;
  assumed: boolean;
};

/** How a turn is run: each one of the agent's own, or left to that computer's defaults. */
export type TurnOptions = { mode?: string; model?: string; effort?: string };

/** A computer this device paired with. `hostKeyHex` is its public key. */
export type PairedHost = {
  hostId: string;
  hostKeyHex: string;
  hostName: string;
  relayUrl: string;
};

export type CodeCheck =
  | { kind: 'empty' }
  | { kind: 'incomplete'; have: number; need: number }
  | { kind: 'complete'; display: string }
  | { kind: 'bad-character'; character: string }
  | { kind: 'too-long' };

export type OfflineReason =
  | 'relay-unreachable'
  | 'host-offline'
  | 'revoked'
  | 'version-mismatch'
  | 'other';

export type ConnState =
  | { kind: 'connecting' }
  | { kind: 'online' }
  | { kind: 'offline'; reason: OfflineReason; message: string; attempt: number }
  | { kind: 'closed' };

export type Project = { name: string; root: string };

export type HistorySession = {
  agent: string;
  externalId: string;
  cwd: string;
  title: string;
  model?: string;
  startedMs: number;
  updatedMs: number;
  messages: number;
  canResume: boolean;
};

export type SharedState = {
  projects: Project[];
  sessions: HistorySession[];
  truncated: boolean;
  /**
   * The sessions that computer has pinned, in the order of its Pinned
   * section; they may be older than everything in `sessions`. Empty from a
   * Leon that does not say.
   */
  pinned: HistorySession[];
  /** Whether that computer has a Den to show; one that does not say must not be asked. */
  den: boolean;
  /** Whether its King can be spoken to; one that does not say must not be asked. */
  king: boolean;
};

/** Whose conversation, in King's service: King's own, or one of its workers'. */
export type KingTarget = 'brain' | { worker: number };

/**
 * A request of King's control protocol, as its service reads it: its name in
 * `op`, its fields in snake case. Only what this app asks is listed.
 */
export type KingRequest =
  | { op: 'status' }
  | { op: 'say'; text: string }
  | { op: 'transcript'; target: KingTarget; since?: number; limit?: number }
  /**
   * The answer's fields sit beside the question's, as the service reads
   * them: `choice` (a permission's choice id, or an option's label),
   * `approve` (a plan), `text` (a reason, words of one's own, what to change).
   */
  | { op: 'answer'; target: KingTarget; question: string; choice?: string; approve?: boolean; text?: string }
  | { op: 'tell_worker'; worker: number; text: string }
  | { op: 'interrupt'; target: KingTarget }
  | { op: 'stop_worker'; worker: number }
  | { op: 'run_start'; task: string; max_parallel?: number; budget_usd?: number }
  | { op: 'run_continue'; run: number; text?: string; budget_usd?: number }
  | { op: 'run_cancel'; run: number };

/** A watch on King's notices; `stop` ends it. */
export type KingWatch = { stop(): void };

/**
 * One lion of a computer's Den: a live agent session there, or a sub-agent of
 * one, with what it is doing as that computer's own Den says it.
 */
export type DenCub = {
  /** The same for as long as its session lives. */
  id: string;
  name: string;
  /** Its agent's tag; empty when not known. */
  agent: string;
  /**
   * `editing`, `reading`, `searching`, `running`, `web`, `delegating`,
   * `planning`, `using-tool`, `thinking`, `mystery`, `waiting-for-user`,
   * `needs-permission`, `idle`, `asleep`, `fainted` or `gone`. One not
   * listed is some kind of working.
   */
  state: string;
  /** The plain fact: the file it edits, the command as typed. */
  detail?: string;
  /** How many tools it has used so far. */
  level: number;
  /** The lion that sent this one out, for a sub-agent. */
  parent?: string;
  /** Nothing but working or quiet is known of it. */
  mystery: boolean;
  /** With `agent`, the session of the shared history it is. */
  externalId?: string;
  cwd?: string;
};

/**
 * A git worktree of a repository over there: its folder, the branch checked
 * out in it (none when the head is detached) and whether it is the main one.
 */
export type Worktree = { path: string; branch?: string; main: boolean };

/**
 * What stands for a project: an image file of its repository, the avatar of
 * the repository's owner (a URL to fetch), or nothing, which is drawn as the
 * folder glyph. The same choice, in the same order, as the desktop.
 */
export type ProjectLogo =
  | { kind: 'image'; format: 'png' | 'jpeg' | 'webp'; base64: string; hash: string }
  | { kind: 'svg'; xml: string; hash: string }
  | { kind: 'avatar'; url: string }
  | { kind: 'none' };

export type TranscriptEntry = { role: string; text: string; atMs: number };

export type TurnRequest = {
  agent: string;
  cwd?: string;
  prompt: string;
  resumeId?: string;
  options: TurnOptions;
  chatId: string;
};

export type PermissionChoice = {
  id: string;
  effect: 'allow-once' | 'allow-always' | 'deny';
  label: string;
  /** How far an "always" reaches, when the agent says. */
  scope?: string;
  /**
   * Whether the agent reads a reason given with it. Without it, what to do
   * instead is said as the next message.
   */
  takesReason: boolean;
};

export type ChoiceQuestion = {
  id: string;
  header?: string;
  question: string;
  options: { label: string; description?: string }[];
  multiSelect: boolean;
  allowOther: boolean;
};

/** What an agent asks the person. */
export type QuestionKind =
  | {
      kind: 'permission';
      tool: string;
      summary: string;
      /** Everything it would pass to the tool, as text. */
      detail: string;
      reason?: string;
      choices: PermissionChoice[];
    }
  | { kind: 'choices'; questions: ChoiceQuestion[] }
  | { kind: 'plan'; plan: string };

export type ChoiceAnswer = { questionId: string; labels: string[]; other?: string };

/** What the person answers. */
export type Answer =
  | { kind: 'permission'; choiceId: string; reason?: string }
  | { kind: 'choices'; answers: ChoiceAnswer[] }
  | { kind: 'plan'; approve: boolean; feedback?: string };

/** The tokens a turn used, as its agent counts them. */
export type TurnTokens = {
  /** Read by the model, cached reads included. */
  input: number;
  /** Of those, read from the provider's cache. */
  cachedInput: number;
  /** Written by the model, its reasoning included. */
  output: number;
  /** Of those, its reasoning, when the agent says. */
  reasoning: number;
  /** How many the model can hold at once, when the agent says. */
  contextWindow?: number;
};

export type TurnEvent =
  | { type: 'started'; sessionId: string; model?: string }
  /** The tokens the turn used so far: each replaces the one before, and the last is the turn's. */
  | { type: 'usage'; tokens: TurnTokens }
  | { type: 'text-delta'; text: string }
  | { type: 'text'; text: string }
  /** More of what the agent thinks before it answers: kept apart, out of sight unless asked for. */
  | { type: 'thought'; text: string }
  /**
   * `detail` is what the one line of `summary` leaves out: the whole command, the change of a
   * file as removed (`- `) and added (`+ `) lines under its path, or the arguments one per line.
   * Empty when the summary says it all.
   */
  | { type: 'tool-use'; id: string; name: string; summary: string; detail: string }
  /** `detail` is everything that came back, as text; empty when the summary says it all. */
  | { type: 'tool-result'; id: string; ok: boolean; summary: string; detail: string }
  | {
      type: 'finished';
      ok: boolean;
      error?: string;
      costUsd?: number;
      durationMs?: number;
      deniedTools: string[];
      /** The person stopped it: `ok` is false and nothing went wrong. */
      interrupted: boolean;
    }
  | { type: 'question'; id: string; ask: QuestionKind }
  | { type: 'question-resolved'; id: string }
  | { type: 'mode-changed'; mode: string }
  | { type: 'disconnected' }
  | { type: 'reconnected' }
  | { type: 'gap' }
  | { type: 'exited'; code: number }
  | { type: 'gone' };

/** A turn on the other computer. `pty` is a decimal string so it can be stored. */
export type RunningTurn = {
  pty: string;
  agent: string;
  chatId: string;
  cwd?: string;
  startedUnix: number;
  exitCode?: number;
};

/**
 * A turn on the other computer. `answer`, `say`, `interrupt`, `setMode` and
 * `setModel` throw a `LinkError` (`NotSupported` for an agent driven one
 * prompt at a time); what they write is kept until the agent has it, across
 * a dropped connection. `cancel` kills the agent.
 */
export type TurnHandle = {
  pty: string;
  cancel(): void;
  answer(questionId: string, answer: Answer): void;
  say(text: string): void;
  interrupt(): void;
  setMode(mode: string): void;
  setModel(model: string): void;
  /** Messages the agent does not have yet. */
  unsent(): number;
};

/** The variants of the library's error, by name. */
export type LinkErrorCode =
  | 'Storage'
  | 'BadCode'
  | 'BadHost'
  | 'RelayUnreachable'
  | 'RelayRefused'
  | 'CodeNotFound'
  | 'CodeExpired'
  | 'CodeBurned'
  | 'CodeUsed'
  | 'WrongCode'
  | 'Denied'
  | 'Timeout'
  | 'Offline'
  | 'Disconnected'
  | 'Closed'
  | 'Host'
  | 'UnknownAgent'
  | 'TurnNotFound'
  | 'UnknownQuestion'
  | 'BadAnswer'
  | 'NotSupported'
  | 'TurnEnded'
  | 'Protocol'
  | 'Internal';

/** What every failing call of the service throws. */
export class LinkError extends Error {
  readonly code: LinkErrorCode;
  /** The detail the library gave, when it gave one. */
  readonly detail?: string;

  constructor(code: LinkErrorCode, detail?: string) {
    super(detail ? `${code}: ${detail}` : code);
    this.name = 'LinkError';
    this.code = code;
    this.detail = detail;
  }
}

/** The connection to one paired computer. */
export interface HostLink {
  state(): ConnState;
  hostName(): string | undefined;
  reconnectNow(): void;
  close(): void;
  shareState(): Promise<SharedState>;
  /** Who is in that computer's Den now. Only when its `SharedState.den` is set. */
  den(): Promise<DenCub[]>;
  transcript(agent: string, externalId: string): Promise<TranscriptEntry[] | undefined>;
  homeDir(): Promise<string>;
  subdirs(path: string): Promise<string[]>;
  /** The worktrees git lists at each folder, in order, main first; none for a folder in no repository. */
  worktrees(roots: string[]): Promise<Worktree[][]>;
  /** The logo of the project rooted at `root`, looked for on the other computer. */
  projectLogo(root: string): Promise<ProjectLogo>;
  installedAgents(): Promise<string[]>;
  /** What `agent` offers on that computer: the agent itself is asked where it can be. */
  agentOptions(agent: string): Promise<AgentOptions>;
  startTurn(request: TurnRequest, onEvent: (event: TurnEvent) => void): Promise<TurnHandle>;
  runningTurns(): Promise<RunningTurn[]>;
  attachTurn(pty: string, onEvent: (event: TurnEvent) => void): Promise<TurnHandle>;
  /**
   * Asks King's service; the answer is the reply's `result`, parsed but not
   * checked (see `@/king/model`). A refusal throws a `LinkError` `Host` with
   * King's sentence. Only when its `SharedState.king` is set.
   */
  kingAsk(request: KingRequest): Promise<unknown>;
  /**
   * Tells `onNotice` every notice of King's service, parsed, until `stop`.
   * `onClosed` says the line to King closed: what was said meanwhile is not
   * known, so the status is to be asked again. It subscribes again by itself.
   */
  kingWatch(onNotice: (notice: unknown) => void, onClosed: (reason: string) => void): KingWatch;
}

/** Everything the application asks of the native module. */
export interface LinkService {
  deviceId(): string;
  deviceName(): string;
  /** Applies to pairings and connections made from now on. */
  setDeviceName(name: string): void;
  agents(): AgentInfo[];
  checkCode(input: string): CodeCheck;
  pair(relayUrl: string, code: string): Promise<PairedHost>;
  connect(host: PairedHost, onState: (state: ConnState) => void): HostLink;
}
