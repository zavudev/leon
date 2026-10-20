import { describe, expect, test } from 'bun:test';

import {
  answerRequest,
  interruptRequest,
  KEPT,
  lastSeq,
  machineLine,
  mergeEntries,
  optionLabels,
  parseEntries,
  parseNotice,
  parseStatus,
  picksOf,
  piecesOf,
  runActions,
  runCancelRequest,
  runContinueRequest,
  runLine,
  runsOn,
  runStartRequest,
  spentLine,
  stopWorkerRequest,
  takesWords,
  tellWorkerRequest,
  workerActions,
  workerWord,
  workersInOrder,
  type KingEntry,
  type KingRun,
  type KingWorker,
} from './model';

// What a computer's King answered, trimmed: the status, then King's
// conversation after entry 95.
const STATUS = JSON.parse(`{
  "conversation": 3,
  "brain": {"agent": "claude", "model": "claude-opus-5-5", "effort": null, "mode": "auto",
    "session_id": "0586c42c", "busy": false, "busy_since_ms": null, "queued": 0,
    "questions": [], "cost_usd": 84.8201976, "last_seq": 99},
  "workers": [
    {"id": 1, "title": "Restyle radar with the brandbook", "project": "radar",
     "cwd": "/home/demo/code/radar-worktrees/feat-brand-style", "job": "code", "tier": "standard",
     "state": "failed",
     "conversation": {"agent": "codex", "model": "gpt-5.6-sol", "effort": null, "mode": null,
       "session_id": "01a1525e", "busy": false, "busy_since_ms": null, "queued": 0,
       "questions": [], "cost_usd": 0.0, "last_seq": 14},
     "last_answer": "The untouched source baseline is now committed as 8a82455.",
     "updated_ms": 1792383362711, "started_in": 1, "check": null, "pull_request": null},
    {"id": 3, "title": "Blog: SEO content from Ahrefs", "project": "blog",
     "cwd": "/home/demo/code/blog-worktrees/feat-blog-seo-content", "job": "code", "tier": null,
     "state": "done",
     "conversation": {"agent": "claude", "model": "sonnet", "effort": "high", "mode": "acceptEdits",
       "session_id": "d3431e75", "busy": false, "busy_since_ms": null, "queued": 0,
       "questions": [], "cost_usd": 11.6046626, "last_seq": 188},
     "last_answer": "Moved the FAQ of the 19 posts into their front matter.",
     "updated_ms": 1792408257115, "started_in": 2, "check": null,
     "pull_request": {"url": "https://example.com/pull/575", "number": 575, "branch": "feat/blog-ui",
       "pushed_ms": 1792408007556, "ci": "failed", "failing": ["Vercel"], "closed": null}}
  ],
  "news": 0, "spent_today_usd": 0.0, "daily_budget_usd": null, "running": [], "runs": []
}`);

const TRANSCRIPT = JSON.parse(`[
  {"seq": 96, "at_ms": 1792408287639, "kind": "agent",
   "text": "Worker #3 (claude, sonnet, high) is done with the content."},
  {"seq": 97, "at_ms": 1792408291828, "kind": "news",
   "text": "[Leon] News of your workers:\\n\\n- #4 has CI failing on its pull request #575."},
  {"seq": 98, "at_ms": 1792408291828, "kind": "report",
   "text": "This answer: 1.2M tokens · 22s · $3.708\\nToday: 63.2M tokens · $93.14",
   "report": {"turn": {"input": 1149344, "cost_usd": 3.7077}, "spent_today_usd": 93.1363738}},
  {"seq": 99, "at_ms": 1792408299470, "kind": "report",
   "text": "This answer: 0 tokens · 0s · $3.288\\nToday: 63.2M tokens · $96.42"}
]`);

function entry(seq: number, kind: KingEntry['kind'] = 'agent', said = `entry ${seq}`): KingEntry {
  return { seq, atMs: seq * 1000, kind, text: said };
}

describe('what King says of itself', () => {
  test('a status reads its brain, its workers and what was spent', () => {
    const status = parseStatus(STATUS)!;
    expect(status.conversation).toBe(3);
    expect(status.brain).toEqual({
      agent: 'claude',
      model: 'claude-opus-5-5',
      effort: undefined,
      busy: false,
      busySinceMs: undefined,
      questions: [],
      costUsd: 84.8201976,
      lastSeq: 99,
    });
    expect(status.workers.map((w) => [w.id, w.state, w.project, runsOn(w.conversation)])).toEqual([
      [1, 'failed', 'radar', 'codex · gpt-5.6-sol'],
      [3, 'done', 'blog', 'claude · sonnet · high'],
    ]);
    expect(status.workers[1]!.lastAnswer).toBe('Moved the FAQ of the 19 posts into their front matter.');
    expect(status.dailyBudgetUsd).toBeUndefined();
    expect(spentLine(status)).toBe('$0.00 today');
    expect(spentLine({ ...status, spentTodayUsd: 3.7077, dailyBudgetUsd: 20 })).toBe('$3.71 of $20.00 today');
    expect(status.runs).toEqual([]);
  });

  test('what is not a status is none, and a status missing its parts is still read', () => {
    expect(parseStatus(null)).toBeUndefined();
    expect(parseStatus('status')).toBeUndefined();
    expect(parseStatus({ workers: [] })).toBeUndefined();
    const bare = parseStatus({ brain: {}, workers: [{ title: 'no id' }, { id: 2 }], runs: 'none' })!;
    expect(bare.brain.agent).toBe('');
    expect(bare.workers.map((w) => [w.id, w.title, w.state])).toEqual([[2, 'Worker #2', 'working']]);
    expect(bare.runs).toEqual([]);
    expect(bare.spentTodayUsd).toBe(0);
  });

  test('the entries of a transcript keep their number, kind and words', () => {
    const entries = parseEntries(TRANSCRIPT);
    expect(entries.map((e) => [e.seq, e.kind])).toEqual([
      [96, 'agent'],
      [97, 'news'],
      [98, 'report'],
      [99, 'report'],
    ]);
    expect(entries[0]!.atMs).toBe(1792408287639);
    expect(lastSeq(entries)).toBe(99);
  });

  test('entries that make no sense are left out, and the rest put in order', () => {
    expect(parseEntries({ seq: 1 })).toEqual([]);
    const entries = parseEntries([
      { seq: 5, kind: 'agent', text: 'five' },
      { seq: 2, kind: 'prompt', text: 'two', at_ms: 9 },
      { seq: 3, kind: 'dance', text: 'an unknown kind' },
      { kind: 'agent', text: 'no number' },
      { seq: 4, kind: 'agent' },
    ]);
    expect(entries).toEqual([
      { seq: 2, atMs: 9, kind: 'prompt', text: 'two' },
      { seq: 5, atMs: 0, kind: 'agent', text: 'five' },
    ]);
  });

  test('a notice is a change with its status, or something for the person', () => {
    const changed = parseNotice({ notice: 'changed', status: STATUS });
    expect(changed?.kind).toBe('changed');
    expect(changed?.kind === 'changed' && changed.status.workers.length).toBe(2);
    expect(parseNotice({ notice: 'notify', title: 'Worker #3 is done', body: 'Its PR is open.' })).toEqual({
      kind: 'notify',
      title: 'Worker #3 is done',
      body: 'Its PR is open.',
      echo: false,
    });
    expect(parseNotice({ notice: 'notify', title: 't', echo: true })).toMatchObject({ echo: true, body: '' });
    expect(parseNotice({ notice: 'changed', status: 'none' })).toBeUndefined();
    expect(parseNotice({ notice: 'dance' })).toBeUndefined();
    expect(parseNotice(undefined)).toBeUndefined();
  });
});

describe('a conversation read a piece at a time', () => {
  test('what is read again replaces what was known, in order of number', () => {
    const have = [entry(1), entry(2), entry(3)];
    const merged = mergeEntries(have, [entry(3, 'agent', 'three again'), entry(5), entry(4)]);
    expect(merged.map((e) => e.seq)).toEqual([1, 2, 3, 4, 5]);
    expect(merged[2]!.text).toBe('three again');
    expect(mergeEntries(have, [])).toBe(have);
    expect(lastSeq([])).toBeUndefined();
  });

  test('only the newest are kept', () => {
    const many = Array.from({ length: KEPT + 10 }, (_, at) => entry(at));
    const merged = mergeEntries([], many);
    expect(merged.length).toBe(KEPT);
    expect(merged[0]!.seq).toBe(10);
  });

  test('tools used one after another are one piece', () => {
    const pieces = piecesOf([
      entry(1, 'prompt'),
      entry(2, 'tool'),
      entry(3, 'tool'),
      entry(4, 'agent'),
      entry(5, 'tool'),
    ]);
    expect(pieces.map((p) => (p.kind === 'tools' ? p.entries.map((e) => e.seq) : p.entry.seq))).toEqual([
      1,
      [2, 3],
      4,
      [5],
    ]);
    expect(pieces.map((p) => p.key)).toEqual(['entry-1', 'tools-2', 'entry-4', 'tools-5']);
  });
});

describe('workers and runs in words', () => {
  test('each state of a worker has its word', () => {
    expect(
      ['working', 'asking', 'done', 'failed', 'paused', 'stopped', 'released', 'new'].map(
        (state) => workerWord(state).word,
      ),
    ).toEqual(['Working', 'Asks', 'Done', 'Failed', 'Stopped, waits', 'Stopped', 'Taken over', 'Working']);
    expect(workerWord('released').mood).toBe('off');
  });

  test('who asks comes first, then who works, then the newest', () => {
    const status = parseStatus({
      brain: {},
      workers: [
        { id: 1, state: 'done', updated_ms: 50 },
        { id: 2, state: 'working', updated_ms: 10 },
        { id: 3, state: 'failed', updated_ms: 90 },
        { id: 4, state: 'asking', updated_ms: 1 },
      ],
    })!;
    expect(workersInOrder(status.workers).map((w) => w.id)).toEqual([4, 2, 3, 1]);
  });

  test('a run says where it is and how far along', () => {
    const run: KingRun = {
      id: 2,
      task: '1a2b3c4d',
      title: 'Checkout',
      project: 'shop',
      state: 'blocked',
      steps: ['done', 'done', 'failed', 'waiting', 'working'],
    };
    expect(runLine(run)).toBe('Run #2 · Blocked · 2 of 5 done');
    const read = parseStatus({ brain: {}, runs: [{ id: 1, state: 'done', steps: [{ state: 'done' }] }] })!;
    expect(read.runs.map(runLine)).toEqual(['Run #1 · Done · 1 of 1 done']);
  });
});

// Questions as King's service lists them in a status: a permission's choices
// are pairs of id and effect, options carry only their questions' ids (their
// labels are in the text), a plan nothing.
const QUESTIONS = JSON.parse(`[
  {"id": "perm-1", "text": "May it use Bash: npm test? Choices: d.0 = allow, d.1 = always, d.2 = deny. Answer with allow, always or deny.",
   "form": {"type": "permission", "choices": [["d.2", "deny"], ["d.0", "allow"], ["d.1", "always"]]}},
  {"id": "ask-1", "text": "Which database? Options: Postgres | SQLite | Postgres, then SQLite.",
   "form": {"type": "choices", "questions": ["q0"]}},
  {"id": "ask-2", "text": "Which? Options: a | b. And where? Options: here | there.",
   "form": {"type": "choices", "questions": ["q0", "q1"]}},
  {"id": "plan-1", "text": "It planned and asks to start:\\n1. Read\\n2. Write", "form": {"type": "plan"}},
  {"id": "odd-1", "text": "Something new", "form": {"type": "riddle"}},
  {"text": "no id", "form": {"type": "plan"}}
]`);

function questions() {
  return parseStatus({ brain: { questions: QUESTIONS } })!.brain.questions;
}

function worker(over: Partial<KingWorker> = {}, busy = false): KingWorker {
  const status = parseStatus({ brain: {}, workers: [{ id: 4, state: 'working', conversation: { busy } }] })!;
  return { ...status.workers[0]!, ...over };
}

describe('what King and its workers ask', () => {
  test('a question is read with how it may be answered; one without an id is left out', () => {
    const [permission, options, two, plan, odd] = questions();
    expect(questions()).toHaveLength(5);
    expect(permission!.form).toEqual({
      kind: 'permission',
      choices: [
        { id: 'd.2', effect: 'deny' },
        { id: 'd.0', effect: 'allow' },
        { id: 'd.1', effect: 'always' },
      ],
    });
    expect(options!.form).toEqual({ kind: 'options', options: ['Postgres', 'SQLite', 'Postgres, then SQLite'] });
    // Options of several questions are not offered: one pick would answer them all.
    expect(two!.form).toEqual({ kind: 'options', options: [] });
    expect(plan!.form).toEqual({ kind: 'plan' });
    expect(odd!.form).toEqual({ kind: 'words' });
  });

  test('the labels of options are read from the end of the text', () => {
    expect(optionLabels('Pick one. Options: yes | no.')).toEqual(['yes', 'no']);
    expect(optionLabels('No options here.')).toEqual([]);
  });

  test('a permission is answered by its choice ids, allow first and deny last', () => {
    const picks = picksOf(questions()[0]!);
    expect(picks.map((p) => [p.label, p.tone, p.reply])).toEqual([
      ['Allow', 'go', { choice: 'd.0' }],
      ['Always', 'plain', { choice: 'd.1' }],
      ['Deny', 'stop', { choice: 'd.2' }],
    ]);
    expect(takesWords(questions()[0]!)).toBe(false);
  });

  test('options are answered by label or in words; a plan is approved or not; the unknown in words', () => {
    const [, options, two, plan, odd] = questions();
    expect(picksOf(options!).map((p) => p.reply)).toEqual([
      { choice: 'Postgres' },
      { choice: 'SQLite' },
      { choice: 'Postgres, then SQLite' },
    ]);
    expect(takesWords(options!)).toBe(true);
    expect(picksOf(two!)).toEqual([]);
    expect(picksOf(plan!).map((p) => [p.label, p.reply])).toEqual([
      ['Approve', { approve: true }],
      ['Reject', { approve: false }],
    ]);
    expect(takesWords(plan!)).toBe(false);
    expect(picksOf(odd!)).toEqual([]);
    expect(takesWords(odd!)).toBe(true);
  });

  test('an answer is the question and its reply side by side, with the words written', () => {
    expect(answerRequest({ worker: 3 }, 'perm-1', { choice: 'd.2' }, '  not on main  ')).toEqual({
      op: 'answer',
      target: { worker: 3 },
      question: 'perm-1',
      choice: 'd.2',
      text: 'not on main',
    });
    expect(answerRequest('brain', 'plan-1', { approve: false }, '   ')).toEqual({
      op: 'answer',
      target: 'brain',
      question: 'plan-1',
      approve: false,
    });
    expect(answerRequest('brain', 'ask-1', {}, 'MySQL')).toEqual({
      op: 'answer',
      target: 'brain',
      question: 'ask-1',
      text: 'MySQL',
    });
    expect(JSON.stringify(answerRequest('brain', 'q', { choice: 'allow' }))).toBe(
      '{"op":"answer","target":"brain","question":"q","choice":"allow"}',
    );
  });
});

describe('what can be done to a worker', () => {
  test('one in a turn can be told more, interrupted and stopped', () => {
    expect(workerActions(worker({}, true))).toEqual({ tell: true, interrupt: true, stop: true });
    expect(workerActions(worker({ state: 'done' }))).toEqual({ tell: true, interrupt: false, stop: true });
  });

  test('one stopped is not stopped again, but can be told more', () => {
    expect(workerActions(worker({ state: 'stopped' }))).toEqual({ tell: true, interrupt: false, stop: false });
  });

  test('one in a terminal of the window cannot be told more; one taken over, nothing', () => {
    expect(workerActions(worker({ terminal: true }))).toEqual({ tell: false, interrupt: false, stop: true });
    expect(workerActions(worker({ state: 'released' }, true))).toEqual({ tell: false, interrupt: false, stop: false });
  });

  test('a worker says the machine it works on, and whether it is in a terminal', () => {
    const read = parseStatus({
      brain: {},
      workers: [
        { id: 1, machine: 'build-box', terminal: true },
        { id: 2, machine: null },
      ],
    })!.workers;
    expect(read.map((w) => [w.machine, w.terminal, machineLine(w)])).toEqual([
      ['build-box', true, 'on build-box'],
      [undefined, false, undefined],
    ]);
  });

  test('the requests to a worker', () => {
    expect(tellWorkerRequest(4, '  and the tests too ')).toEqual({ op: 'tell_worker', worker: 4, text: 'and the tests too' });
    expect(interruptRequest({ worker: 4 })).toEqual({ op: 'interrupt', target: { worker: 4 } });
    expect(interruptRequest('brain')).toEqual({ op: 'interrupt', target: 'brain' });
    expect(stopWorkerRequest(4)).toEqual({ op: 'stop_worker', worker: 4 });
  });
});

describe('what can be done to a run', () => {
  const run = (state: string): KingRun => ({ id: 7, task: 'abc', title: '', project: '', state, steps: [] });

  test('a blocked run goes on or is called off; a running one only called off; one over, nothing', () => {
    expect(runActions(run('blocked'))).toEqual({ continue: true, cancel: true });
    expect(runActions(run('running'))).toEqual({ continue: false, cancel: true });
    expect(runActions(run('done'))).toEqual({ continue: false, cancel: false });
    expect(runActions(run('canceled'))).toEqual({ continue: false, cancel: false });
  });

  test('a run keeps its task', () => {
    const read = parseStatus({ brain: {}, runs: [{ id: 1, task: '1a2b3c4d', state: 'blocked' }] })!;
    expect(read.runs[0]!.task).toBe('1a2b3c4d');
  });

  test('the requests of runs', () => {
    expect(runStartRequest('  #1a2b3c4d ')).toEqual({ op: 'run_start', task: '1a2b3c4d' });
    expect(runStartRequest('   ')).toBeUndefined();
    expect(runContinueRequest(7)).toEqual({ op: 'run_continue', run: 7 });
    expect(runContinueRequest(7, ' skip the flaky test ')).toEqual({
      op: 'run_continue',
      run: 7,
      text: 'skip the flaky test',
    });
    expect(runContinueRequest(7, '  ')).toEqual({ op: 'run_continue', run: 7 });
    expect(runCancelRequest(7)).toEqual({ op: 'run_cancel', run: 7 });
  });
});
