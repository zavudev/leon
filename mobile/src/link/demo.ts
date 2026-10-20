// A computer that is not there: what the app shows before anything is paired,
// so that it can be tried (and reviewed) without a Leon to connect to. Its
// projects, sessions and Den are made up, and its agent answers every prompt
// with the same short, scripted turn; its King answers in the same way.
// Nothing leaves the phone.
import {
  LinkError,
  type AgentOptions,
  type ConnState,
  type DenCub,
  type HistorySession,
  type HostLink,
  type KingRequest,
  type KingWatch,
  type PairedHost,
  type SharedState,
  type TranscriptEntry,
  type TurnEvent,
  type TurnHandle,
} from './types';

export const DEMO_HOST_ID = 'demo';

export const DEMO_HOST: PairedHost = {
  hostId: DEMO_HOST_ID,
  hostKeyHex: '',
  hostName: 'Demo computer',
  relayUrl: 'made up, on this phone',
};

export function isDemo(hostId: string): boolean {
  return hostId === DEMO_HOST_ID;
}

const HOME = '/home/demo';
const SHOP = `${HOME}/code/shop`;
const API = `${HOME}/code/api`;
const MIN = 60_000;

function session(
  agent: string,
  externalId: string,
  cwd: string,
  title: string,
  ago: number,
  messages: number,
): HistorySession {
  const updatedMs = Date.now() - ago * MIN;
  return {
    agent,
    externalId,
    cwd,
    title,
    startedMs: updatedMs - 40 * MIN,
    updatedMs,
    messages,
    canResume: true,
  };
}

function sessions(): HistorySession[] {
  return [
    session('claude', 'demo-checkout', SHOP, 'Fix the rounding of totals at checkout', 2, 14),
    session('codex', 'demo-rate-limit', API, 'Add rate limiting to the public endpoints', 6, 22),
    session('opencode', 'demo-readme', SHOP, 'Update the README with the new setup steps', 35, 8),
    session('claude', 'demo-flaky', API, 'Find why the orders test is flaky', 180, 31),
    session('claude', 'demo-dark', SHOP, 'Dark mode for the product page', 1500, 19),
  ];
}

const TRANSCRIPTS: Record<string, [string, string][]> = {
  'demo-checkout': [
    ['user', 'The totals at checkout are off by a cent sometimes. Can you find out why?'],
    [
      'assistant',
      'The cart adds prices as floating-point numbers, so 0.1 + 0.2 becomes 0.30000000000000004 before rounding. I changed `cartTotal` to add whole cents and only format at the end, and added a test with the three prices that failed.',
    ],
  ],
  'demo-rate-limit': [
    ['user', 'Add rate limiting to the public endpoints, 60 requests a minute per key.'],
    [
      'assistant',
      'Done: a token bucket per API key in `middleware/limit.ts`, applied to every route under `/v1/public`. Requests over the limit get a 429 with a `Retry-After` header.',
    ],
  ],
};

function den(): DenCub[] {
  return [
    {
      id: 'demo-1',
      name: 'Rounding',
      agent: 'claude',
      state: 'needs-permission',
      detail: 'npm test -- cart',
      level: 12,
      mystery: false,
      externalId: 'demo-checkout',
      cwd: SHOP,
    },
    {
      id: 'demo-2',
      name: 'Limits',
      agent: 'codex',
      state: 'editing',
      detail: 'src/middleware/limit.ts',
      level: 20,
      mystery: false,
      externalId: 'demo-rate-limit',
      cwd: API,
    },
    {
      id: 'demo-3',
      name: 'Scout',
      agent: 'claude',
      state: 'searching',
      detail: 'cartTotal',
      level: 3,
      parent: 'demo-1',
      mystery: false,
      cwd: SHOP,
    },
    {
      id: 'demo-4',
      name: 'Docs',
      agent: 'opencode',
      state: 'idle',
      level: 5,
      mystery: false,
      externalId: 'demo-readme',
      cwd: SHOP,
    },
  ];
}

function options(agent: string): AgentOptions {
  return {
    modes: [
      { id: 'default', label: 'Ask before acting', dangerous: false },
      { id: 'acceptEdits', label: 'Accept edits', dangerous: false },
    ],
    defaultMode: 'default',
    models: [
      {
        id: agent === 'codex' ? 'gpt-5' : 'default',
        label: agent === 'codex' ? 'GPT-5' : 'Default',
        efforts: ['low', 'medium', 'high'],
      },
    ],
    questions: true,
    followUps: false,
    liveChanges: false,
    assumed: false,
  };
}

const wait = (ms: number) => new Promise((done) => setTimeout(done, ms));

const REPLY =
  'This is the demo computer, so nothing ran for real. On your own computer the agent would now work in the project folder, and you would follow its steps here, answer what it asks, and get its answer on this phone.';

/** The scripted turn: it reads, asks to run a command, then answers. */
function runTurn(prompt: string, onEvent: (event: TurnEvent) => void): TurnHandle {
  let over = false;
  let answered: ((allowed: boolean) => void) | null = null;
  const say = (event: TurnEvent) => {
    if (!over) onEvent(event);
  };
  const end = (event: TurnEvent) => {
    say(event);
    say({ type: 'exited', code: 0 });
    over = true;
  };

  void (async () => {
    say({ type: 'started', sessionId: `demo-${Date.now().toString(36)}`, model: 'Demo' });
    await wait(600);
    say({ type: 'tool-use', id: 't1', name: 'Read', summary: 'README.md', detail: '' });
    await wait(700);
    say({ type: 'tool-result', id: 't1', ok: true, summary: '42 lines', detail: '' });
    await wait(400);
    say({
      type: 'question',
      id: 'q1',
      ask: {
        kind: 'permission',
        tool: 'Bash',
        summary: 'npm test',
        detail: 'npm test',
        reason: 'To check that everything still passes.',
        choices: [
          { id: 'allow', effect: 'allow-once', label: 'Allow', takesReason: false },
          { id: 'deny', effect: 'deny', label: 'Deny', takesReason: true },
        ],
      },
    });
    const allowed = await new Promise<boolean>((done) => {
      answered = done;
    });
    say({ type: 'question-resolved', id: 'q1' });
    if (allowed) {
      say({ type: 'tool-use', id: 't2', name: 'Bash', summary: 'npm test', detail: 'npm test' });
      await wait(1200);
      say({ type: 'tool-result', id: 't2', ok: true, summary: '18 passed', detail: '' });
    }
    await wait(300);
    const text = `You asked: “${prompt.trim().slice(0, 120)}”.\n\n${REPLY}`;
    for (const word of text.split(/(?<= )/)) {
      if (over) return;
      say({ type: 'text-delta', text: word });
      await wait(25);
    }
    say({ type: 'text', text });
    end({
      type: 'finished',
      ok: true,
      durationMs: 4_000,
      deniedTools: allowed ? [] : ['Bash'],
      interrupted: false,
    });
  })();

  return {
    pty: `demo-${Date.now()}`,
    cancel() {
      end({ type: 'finished', ok: false, deniedTools: [], interrupted: true });
    },
    answer(_questionId, reply) {
      const allowed = reply.kind === 'permission' && reply.choiceId === 'allow';
      answered?.(allowed);
      answered = null;
    },
    say() {
      throw new LinkError('NotSupported');
    },
    interrupt() {
      throw new LinkError('NotSupported');
    },
    setMode() {
      throw new LinkError('NotSupported');
    },
    setModel() {
      throw new LinkError('NotSupported');
    },
    unsent: () => 0,
  };
}

/** One entry of a made-up conversation, as King's service sends it. */
type DemoEntry = { seq: number; at_ms: number; kind: string; text: string };

const KING_REPLY =
  'This is the demo computer, so I started nobody. On your own computer I would split this into tasks, give each to a worker in its own worktree, check what they did, and tell you here when it is ready.';

/**
 * King of the demo computer: three workers (one asks to run a command, one
 * works on another machine, one in a terminal of the window) and two runs
 * (one blocked), made up, and a conversation it adds to when spoken to. It
 * answers the way King's service does, in its own JSON, and refuses with its
 * sentences, so the app reads it as it reads the real one.
 */
function demoKing() {
  const start = Date.now() - 50 * MIN;
  const brain: DemoEntry[] = [];
  const say = (into: DemoEntry[], kind: string, text: string, at = Date.now()) => {
    into.push({ seq: (into.at(-1)?.seq ?? -1) + 1, at_ms: at, kind, text });
  };
  say(brain, 'prompt', 'Fix the rounding at checkout, and add rate limits to the public API.', start);
  say(brain, 'tool', 'start_worker: Fix the rounding of totals at checkout', start + MIN);
  say(brain, 'tool', 'start_worker: Rate limits for the public endpoints', start + MIN);
  say(
    brain,
    'agent',
    'Two workers are on it: **#1** fixes the rounding in `shop`, **#2** adds the limits in `api`. I will check their work and tell you when each is ready.',
    start + 2 * MIN,
  );
  say(brain, 'news', '[Leon] News of your workers:\n\n- #1 "Fix the rounding of totals at checkout" is done.', start + 30 * MIN);
  say(
    brain,
    'agent',
    'Worker #1 is done: totals are added in whole cents now, with a test for the three prices that failed. I read the change and the tests pass.',
    start + 31 * MIN,
  );
  const workers: Record<number, DemoEntry[]> = { 1: [], 2: [], 3: [] };
  say(workers[1]!, 'prompt', 'The totals at checkout are off by a cent sometimes. Find out why and fix it.', start + MIN);
  say(workers[1]!, 'tool', 'Read: src/cart.ts', start + 2 * MIN);
  say(workers[1]!, 'tool', 'Bash: npm test -- cart', start + 3 * MIN);
  say(workers[1]!, 'agent', 'Fixed: `cartTotal` adds whole cents and formats only at the end.', start + 29 * MIN);
  say(workers[2]!, 'prompt', 'Add rate limiting to the public endpoints, 60 requests a minute per key.', start + MIN);
  say(workers[2]!, 'tool', 'Edit: src/middleware/limit.ts', start + 20 * MIN);

  say(workers[2]!, 'question', 'May it use Bash: npm install express-rate-limit?', start + 21 * MIN);
  say(workers[3]!, 'prompt', 'Write the changelog entry for the rounding fix, in the terminal.', start + 32 * MIN);

  let busySince: number | null = null;
  let spent = 1.84;
  const listeners = new Set<(notice: unknown) => void>();
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const later = (ms: number, run: () => void) => {
    const timer = setTimeout(() => {
      timers.delete(timer);
      run();
    }, ms);
    timers.add(timer);
  };

  /** A question waiting, as the service lists it. */
  type DemoQuestion = { id: string; text: string; form: unknown };
  /** A worker, as the demo keeps it: what the status says of it, and what it was told. */
  type DemoWorker = {
    id: number;
    title: string;
    project: string;
    cwd: string;
    machine: string | null;
    terminal: boolean;
    state: string;
    agent: string;
    model: string;
    busySince: number | null;
    questions: DemoQuestion[];
    cost: number;
    lastAnswer: string | null;
    updated: number;
  };
  const team: DemoWorker[] = [
    {
      id: 1,
      title: 'Fix the rounding of totals at checkout',
      project: 'shop',
      cwd: SHOP,
      machine: null,
      terminal: false,
      state: 'done',
      agent: 'claude',
      model: 'sonnet',
      busySince: null,
      questions: [],
      cost: 0.41,
      lastAnswer: 'Fixed: `cartTotal` adds whole cents and formats only at the end.',
      updated: start + 29 * MIN,
    },
    {
      id: 2,
      title: 'Rate limits for the public endpoints',
      project: 'api',
      cwd: API,
      machine: 'build-box',
      terminal: false,
      state: 'asking',
      agent: 'codex',
      model: 'gpt-5',
      busySince: start + 20 * MIN,
      questions: [
        {
          id: 'perm-1',
          text: 'May it use Bash: npm install express-rate-limit? Choices: d.0 = allow, d.1 = always, d.2 = deny. Answer with allow, always or deny.',
          form: { type: 'permission', choices: [['d.0', 'allow'], ['d.1', 'always'], ['d.2', 'deny']] },
        },
      ],
      cost: 0.31,
      lastAnswer: null,
      updated: start + 21 * MIN,
    },
    {
      id: 3,
      title: 'Changelog for the rounding fix',
      project: 'shop',
      cwd: SHOP,
      machine: null,
      terminal: true,
      state: 'working',
      agent: 'claude',
      model: 'haiku',
      busySince: null,
      questions: [],
      cost: 0,
      lastAnswer: null,
      updated: start + 32 * MIN,
    },
  ];
  /** A run, as the demo keeps it. */
  type DemoRun = { id: number; task: string; title: string; project: string; state: string; steps: string[]; note: string | null };
  const runs: DemoRun[] = [
    {
      id: 1,
      task: '7f3a91c2',
      title: 'Checkout and limits',
      project: 'shop',
      state: 'running',
      steps: ['done', 'working', 'waiting'],
      note: null,
    },
    {
      id: 2,
      task: 'c04be8d1',
      title: 'Dark mode for the dashboard',
      project: 'api',
      state: 'blocked',
      steps: ['done', 'failed', 'waiting'],
      note: 'step 2 failed: its check kept failing (3 tries)',
    },
  ];
  const brainQuestions: DemoQuestion[] = [];

  const conversation = (
    agent: string,
    model: string,
    entries: DemoEntry[],
    since: number | null,
    questions: DemoQuestion[],
    cost: number,
  ) => ({
    agent,
    model,
    effort: null,
    busy: since !== null,
    busy_since_ms: since,
    queued: 0,
    questions,
    cost_usd: cost,
    last_seq: entries.at(-1)?.seq ?? null,
  });

  const status = () => ({
    conversation: 1,
    brain: conversation('claude', 'opus', brain, busySince, brainQuestions, 1.12),
    workers: team.map((w) => ({
      id: w.id,
      title: w.title,
      project: w.project,
      cwd: w.cwd,
      machine: w.machine,
      terminal: w.terminal,
      state: w.state,
      conversation: conversation(w.agent, w.model, workers[w.id]!, w.busySince, w.questions, w.cost),
      last_answer: w.lastAnswer,
      updated_ms: w.updated,
    })),
    news: 0,
    spent_today_usd: spent,
    daily_budget_usd: 10,
    runs: runs.map((run) => ({ ...run, steps: run.steps.map((state) => ({ state })) })),
  });

  const tell = (notice: unknown) => {
    for (const listener of listeners) listener(notice);
  };
  const changed = () => tell({ notice: 'changed', status: status() });
  const refuse = (sentence: string): never => {
    throw new LinkError('Host', sentence);
  };
  /** A worker that is King's to command, as the service finds it. */
  const owned = (id: number): DemoWorker => {
    const found = team.find((w) => w.id === id) ?? refuse(`there is no worker #${id}`);
    if (found.state === 'released') refuse(`worker #${id} was taken over by the person`);
    return found;
  };
  /** A run that is not over. */
  const going = (id: number): DemoRun => {
    const found = runs.find((r) => r.id === id) ?? refuse(`there is no run #${id}`);
    if (found.state === 'done' || found.state === 'canceled') refuse(`run #${id} is over`);
    return found;
  };
  /** A worker's turn, made up: it says it did what it was told, a moment later. */
  const turn = (w: DemoWorker, answer: string) => {
    w.state = 'working';
    w.busySince = Date.now();
    w.updated = Date.now();
    later(2_000, () => {
      if (w.busySince === null) return;
      say(workers[w.id]!, 'agent', answer);
      w.state = 'done';
      w.busySince = null;
      w.lastAnswer = answer;
      w.cost += 0.04;
      w.updated = Date.now();
      changed();
    });
  };

  return {
    async ask(request: KingRequest): Promise<unknown> {
      await wait(150);
      switch (request.op) {
        case 'status':
          return status();
        case 'transcript': {
          const entries = request.target === 'brain' ? brain : workers[request.target.worker];
          if (!entries) throw new LinkError('Host', 'no such worker');
          const after = entries.filter((e) => request.since === undefined || e.seq > request.since);
          return after.slice(-(request.limit ?? after.length));
        }
        case 'say': {
          say(brain, 'prompt', request.text);
          busySince = Date.now();
          changed();
          later(900, () => {
            say(brain, 'tool', 'list_workers');
            changed();
          });
          later(2_400, () => {
            say(brain, 'agent', KING_REPLY);
            busySince = null;
            spent += 0.07;
            changed();
            tell({ notice: 'notify', title: 'Worker #2 is checking', body: 'Rate limits for the public endpoints.', echo: false });
          });
          return null;
        }
        case 'answer': {
          const { target, question } = request;
          const w = target === 'brain' ? undefined : owned(target.worker);
          const waiting = w ? w.questions : brainQuestions;
          const open = waiting.find((q) => q.id === question) ?? refuse(`nothing waits on a question "${question}"`);
          const form = (open.form as { type?: string }).type;
          if (form === 'permission' && request.choice === undefined) refuse('a permission is answered with a choice');
          if (form === 'plan' && request.approve === undefined) refuse('a plan is answered with approve true or false');
          if (form === 'choices' && request.choice === undefined && !request.text?.trim()) {
            refuse('options are answered with a choice or with text');
          }
          waiting.splice(waiting.indexOf(open), 1);
          if (w) {
            const denied = request.choice === 'd.2' || request.choice === 'deny';
            say(workers[w.id]!, 'tool', `${denied ? 'Denied' : 'Allowed'}: ${open.text.split('?')[0]}`);
            if (denied) {
              w.state = 'paused';
              w.busySince = null;
              w.updated = Date.now();
            } else {
              turn(w, 'Installed `express-rate-limit` and limited the public routes to 60 requests a minute per key.');
            }
          }
          changed();
          return null;
        }
        case 'tell_worker': {
          const w = owned(request.worker);
          if (!request.text.trim()) refuse('there is nothing to say');
          if (w.terminal) {
            refuse(
              "it works in a terminal of Leon's window, where the person sees it: it cannot be told more from here; ask the person, or stop it and start another",
            );
          }
          say(workers[w.id]!, 'prompt', request.text.trim());
          turn(w, 'Done: this is the demo computer, so nothing was changed.');
          changed();
          return null;
        }
        case 'interrupt': {
          const w = request.target === 'brain' ? undefined : owned(request.target.worker);
          const busy = w ? w.busySince !== null : busySince !== null;
          if (!busy) refuse('it is not in a turn');
          if (w) {
            // A turn that ends drops what it asked.
            w.busySince = null;
            w.questions = [];
            w.state = 'paused';
            w.updated = Date.now();
          } else {
            busySince = null;
          }
          changed();
          return null;
        }
        case 'stop_worker': {
          const w = owned(request.worker);
          w.state = 'stopped';
          w.busySince = null;
          w.questions = [];
          w.updated = Date.now();
          changed();
          return null;
        }
        case 'run_start': {
          const task = request.task.trim();
          if (!task) refuse('which task?');
          const run: DemoRun = {
            id: (runs.at(-1)?.id ?? 0) + 1,
            task,
            title: `Task ${task}`,
            project: 'shop',
            state: 'running',
            steps: ['working', 'waiting'],
            note: null,
          };
          runs.push(run);
          changed();
          return { run: run.id };
        }
        case 'run_continue': {
          const run = going(request.run);
          run.state = 'running';
          run.note = null;
          run.steps = run.steps.map((step) => (step === 'failed' ? 'working' : step));
          changed();
          later(2_500, () => {
            if (run.state !== 'running') return;
            run.steps = run.steps.map(() => 'done');
            run.state = 'done';
            changed();
          });
          return null;
        }
        case 'run_cancel': {
          const run = going(request.run);
          run.state = 'canceled';
          run.note = null;
          run.steps = run.steps.map((step) => (step === 'done' ? step : 'canceled'));
          changed();
          return null;
        }
      }
    },
    watch(onNotice: (notice: unknown) => void): KingWatch {
      listeners.add(onNotice);
      return { stop: () => listeners.delete(onNotice) };
    },
    close() {
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
      listeners.clear();
    },
  };
}

/** The connection to the demo computer: online a moment after it is opened. */
export function createDemoLink(onState: (state: ConnState) => void): HostLink {
  let state: ConnState = { kind: 'connecting' };
  const timer = setTimeout(() => {
    state = { kind: 'online' };
    onState(state);
  }, 300);
  queueMicrotask(() => onState(state));
  const king = demoKing();

  return {
    state: () => state,
    hostName: () => DEMO_HOST.hostName,
    reconnectNow() {},
    close() {
      clearTimeout(timer);
      king.close();
      state = { kind: 'closed' };
    },
    async shareState(): Promise<SharedState> {
      const all = sessions();
      return {
        projects: [
          { name: 'shop', root: SHOP },
          { name: 'api', root: API },
        ],
        sessions: all,
        truncated: false,
        pinned: all.filter((s) => s.externalId === 'demo-rate-limit' || s.externalId === 'demo-dark'),
        den: true,
        king: true,
      };
    },
    den: async () => den(),
    async transcript(_agent, externalId): Promise<TranscriptEntry[] | undefined> {
      const said = TRANSCRIPTS[externalId];
      const start = Date.now() - 30 * MIN;
      return (said ?? [['user', 'A session of the demo computer.']]).map(([role, text], at) => ({
        role,
        text,
        atMs: start + at * MIN,
      }));
    },
    homeDir: async () => HOME,
    async subdirs(path) {
      if (path === HOME) return ['code'];
      if (path === `${HOME}/code`) return ['api', 'shop'];
      return [];
    },
    worktrees: async (roots) => roots.map((root) => [{ path: root, branch: 'main', main: true }]),
    projectLogo: async () => ({ kind: 'none' }),
    installedAgents: async () => ['claude', 'codex', 'opencode'],
    agentOptions: async (agent) => options(agent),
    startTurn: async (request, onEvent) => runTurn(request.prompt, onEvent),
    runningTurns: async () => [],
    async attachTurn() {
      throw new LinkError('TurnNotFound');
    },
    kingAsk: (request) => king.ask(request),
    kingWatch: (onNotice) => king.watch(onNotice),
  };
}
