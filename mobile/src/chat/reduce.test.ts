import { describe, expect, test } from 'bun:test';

import type { AgentOptions, QuestionKind, TurnEvent } from '@/link/types';

import { answerLine, fitOptions, optionsLine, parseTodos, refitStored, withoutLegacyMode } from './model';
import {
  initialTurn,
  isOver,
  isWaiting,
  markAnswered,
  pendingQuestions,
  reduceTurn,
  sayMore,
  type TurnState,
} from './reduce';

function run(events: TurnEvent[], from: TurnState = initialTurn('t1')): TurnState {
  return events.reduce((state, event, index) => reduceTurn(state, event, 1000 + index), from);
}

const FINISHED: TurnEvent = { type: 'finished', ok: true, deniedTools: [], interrupted: false };

describe('reduceTurn', () => {
  test('deltas accumulate into one streaming row', () => {
    const state = run([
      { type: 'started', sessionId: 's1', model: 'opus' },
      { type: 'text-delta', text: 'Hel' },
      { type: 'text-delta', text: 'lo' },
    ]);
    expect(state.sessionId).toBe('s1');
    expect(state.model).toBe('opus');
    expect(state.items).toEqual([
      { id: 't1:0', kind: 'assistant', text: 'Hello', at: 1001, streaming: true },
    ]);
    expect(state.draftId).toBe('t1:0');
  });

  test('a whole message replaces the draft instead of adding a row', () => {
    const state = run([
      { type: 'text-delta', text: 'Hel' },
      { type: 'text', text: 'Hello.' },
    ]);
    expect(state.items).toEqual([
      { id: 't1:0', kind: 'assistant', text: 'Hello.', at: 1000, streaming: false },
    ]);
    expect(state.draftId).toBeNull();
  });

  test('a whole message without a draft is its own row', () => {
    const state = run([
      { type: 'text', text: 'One.' },
      { type: 'text', text: 'Two.' },
    ]);
    expect(state.items.map((m) => m.kind === 'assistant' && m.text)).toEqual(['One.', 'Two.']);
  });

  test('a tool closes the draft, runs, then reports', () => {
    const running = run([
      { type: 'text-delta', text: 'Looking' },
      { type: 'tool-use', id: 'a', name: 'Bash', summary: 'ls', detail: '' },
    ]);
    expect(running.draftId).toBeNull();
    expect(running.items[0]).toMatchObject({ kind: 'assistant', streaming: false });
    expect(running.items[1]).toMatchObject({ kind: 'tool', name: 'Bash', status: 'running' });

    const done = run([{ type: 'tool-result', id: 'a', ok: false, summary: 'exit 2', detail: '' }], running);
    expect(done.items[1]).toMatchObject({ status: 'error', result: 'exit 2' });

    const after = run([{ type: 'text-delta', text: 'So' }], done);
    expect(after.items).toHaveLength(3);
    expect(after.items[2]).toMatchObject({ id: 't1:2', kind: 'assistant', text: 'So' });
  });

  test('a result for an unknown tool changes nothing', () => {
    const state = run([{ type: 'text', text: 'x' }]);
    expect(run([{ type: 'tool-result', id: 'nope', ok: true, summary: '', detail: '' }], state)).toBe(state);
  });

  test('denied tools become one warning that names them once each', () => {
    const state = run([
      { type: 'text', text: 'I could not.' },
      { type: 'finished', ok: true, deniedTools: ['Write', 'Bash', 'Write'], interrupted: false },
    ]);
    expect(state.status).toBe('finished');
    expect(state.items[1]).toMatchObject({
      kind: 'notice',
      tone: 'warning',
      text: 'The agent was not allowed to use Write and Bash.',
      deniedTools: ['Write', 'Bash'],
    });
  });

  test('a failed finish carries its error, also when the library made it up', () => {
    // What the library synthesises when the agent is not installed.
    const state = run([
      { type: 'finished', ok: false, error: 'command not found: claude', deniedTools: [], interrupted: false },
      { type: 'exited', code: 127 },
    ]);
    expect(state.items).toEqual([
      { id: 't1:0', kind: 'notice', tone: 'error', text: 'command not found: claude', at: 1000, deniedTools: undefined },
    ]);
    expect(isOver(state)).toBe(true);
  });

  test('a failed finish without a reason still says something', () => {
    const state = run([{ type: 'finished', ok: false, deniedTools: [], interrupted: false }]);
    expect(state.items[0]).toMatchObject({ tone: 'error', text: 'The agent stopped with an error.' });
  });

  test('finishing settles the draft and keeps cost and duration', () => {
    const state = run([
      { type: 'text-delta', text: 'Done' },
      { type: 'finished', ok: true, costUsd: 0.02, durationMs: 1500, deniedTools: [], interrupted: false },
    ]);
    expect(state.items[0]).toMatchObject({ streaming: false });
    expect(state).toMatchObject({ ok: true, costUsd: 0.02, durationMs: 1500, draftId: null });
    expect(isOver(state)).toBe(false);
  });

  test('a second finish is ignored', () => {
    const once = run([{ type: 'finished', ok: false, error: 'boom', deniedTools: [], interrupted: false }]);
    expect(run([FINISHED], once)).toBe(once);
  });

  test('exit after a finish ends the turn without another row', () => {
    const state = run([{ type: 'text', text: 'ok' }, FINISHED, { type: 'exited', code: 0 }]);
    expect(state.items).toHaveLength(1);
    expect(state.status).toBe('ended');
  });

  test('exit without a finish marks the turn stopped and its running tools too', () => {
    const state = run([
      { type: 'tool-use', id: 'a', name: 'Bash', summary: 'sleep 100', detail: '' },
      { type: 'exited', code: 129 },
    ]);
    expect(state.items[0]).toMatchObject({ kind: 'tool', status: 'stopped' });
    expect(state.items[1]).toMatchObject({ kind: 'notice', tone: 'info', text: 'Stopped.' });
  });

  test('a turn that is gone says so', () => {
    const state = run([{ type: 'gone' }]);
    expect(state.items[0]).toMatchObject({ tone: 'warning' });
    expect(isOver(state)).toBe(true);
  });

  test('losing and regaining the connection adds no rows', () => {
    const lost = run([{ type: 'text-delta', text: 'a' }, { type: 'disconnected' }]);
    expect(lost.disconnected).toBe(true);
    const back = run([{ type: 'reconnected' }, { type: 'text-delta', text: 'b' }], lost);
    expect(back.disconnected).toBe(false);
    expect(back.items).toEqual([
      { id: 't1:0', kind: 'assistant', text: 'ab', at: 1000, streaming: true },
    ]);
  });

  test('a gap is said where it happened', () => {
    const state = run([{ type: 'gap' }, { type: 'text', text: 'tail' }]);
    expect(state.items[0]).toMatchObject({ kind: 'notice', tone: 'info' });
    expect(state.items[1]).toMatchObject({ kind: 'assistant', text: 'tail' });
  });

  test('replaying a turn from its start rebuilds the same rows with the same ids', () => {
    const events: TurnEvent[] = [
      { type: 'started', sessionId: 's1' },
      { type: 'text-delta', text: 'Let me look.' },
      { type: 'tool-use', id: 'a', name: 'Read', summary: 'README.md', detail: '' },
      { type: 'tool-result', id: 'a', ok: true, summary: '', detail: '' },
      { type: 'text-delta', text: 'It says' },
      { type: 'text', text: 'It says hello.' },
      FINISHED,
      { type: 'exited', code: 0 },
    ];
    const live = run(events.slice(0, 5));
    const replayed = run(events);
    const direct = run(events, initialTurn('t1'));
    expect(replayed).toEqual(direct);
    // What was on screen before the replay keeps its identity.
    expect(replayed.items.slice(0, 2).map((m) => m.id)).toEqual(
      live.items.slice(0, 2).map((m) => m.id),
    );
    expect(replayed.items.map((m) => m.id)).toEqual(['t1:0', 't1:1', 't1:2']);
  });
});

const ASK: QuestionKind = {
  kind: 'permission',
  tool: 'Bash',
  summary: 'make test',
  detail: '{ "command": "make test" }',
  choices: [
    { id: 'once', effect: 'allow-once', label: 'Allow once', takesReason: false },
    { id: 'always.0', effect: 'allow-always', label: 'Always allow Bash(make test)', scope: 'this session', takesReason: false },
    { id: 'deny', effect: 'deny', label: 'Deny', takesReason: true },
  ],
};

describe('questions', () => {
  test('a question is a row that waits, and closes the text before it', () => {
    const state = run([
      { type: 'text-delta', text: 'Let me run the tests' },
      { type: 'question', id: 'q1', ask: ASK },
    ]);
    expect(state.items[0]).toMatchObject({ kind: 'assistant', streaming: false });
    expect(state.items[1]).toMatchObject({ kind: 'question', questionId: 'q1', status: 'open' });
    expect(isWaiting(state)).toBe(true);
    expect(pendingQuestions(state).map((q) => q.questionId)).toEqual(['q1']);
  });

  test('answered here it is on its way, and answered once the agent has it', () => {
    let state = run([{ type: 'question', id: 'q1', ask: ASK }]);
    state = markAnswered(state, 'q1', { kind: 'permission', choiceId: 'always.0' });
    expect(state.items[0]).toMatchObject({ status: 'sending', chosen: 'Always allow Bash(make test)' });
    // Nobody is waited for any more, but it is still pending delivery.
    expect(isWaiting(state)).toBe(false);
    expect(pendingQuestions(state)).toHaveLength(1);
    state = run([{ type: 'question-resolved', id: 'q1' }], state);
    expect(state.items[0]).toMatchObject({ status: 'answered', chosen: 'Always allow Bash(make test)' });
    expect(pendingQuestions(state)).toHaveLength(0);
    // Answering again changes nothing that is settled.
    expect(markAnswered(state, 'q1', { kind: 'permission', choiceId: 'deny' })).toEqual(state);
  });

  test('a question the agent stopped waiting for is closed without an answer', () => {
    const withdrawn = run([
      { type: 'question', id: 'q1', ask: ASK },
      { type: 'question-resolved', id: 'q1' },
    ]);
    expect(withdrawn.items[0]).toMatchObject({ status: 'closed' });
    expect((withdrawn.items[0] as { chosen?: string }).chosen).toBeUndefined();

    const ended = run([{ type: 'question', id: 'q1', ask: ASK }, { type: 'exited', code: 1 }]);
    expect(ended.items[0]).toMatchObject({ kind: 'question', status: 'closed' });
    expect(isWaiting(ended)).toBe(false);
  });

  test('a replay rebuilds the same rows, the open question included', () => {
    const events: TurnEvent[] = [
      { type: 'started', sessionId: 's1' },
      { type: 'question', id: 'q1', ask: ASK },
      { type: 'question-resolved', id: 'q1' },
      { type: 'tool-use', id: 't', name: 'Bash', summary: 'make test', detail: '' },
      { type: 'question', id: 'q2', ask: ASK },
    ];
    const first = run(events);
    const replayed = run(events);
    expect(replayed.items.map((m) => m.id)).toEqual(first.items.map((m) => m.id));
    expect(pendingQuestions(replayed).map((q) => q.questionId)).toEqual(['q2']);
    // Asked again after a gap: still one row.
    const again = run([{ type: 'question', id: 'q2', ask: ASK }], replayed);
    expect(again.items).toHaveLength(replayed.items.length);
  });

  test('stopping it is said as a stop, not as an error', () => {
    const state = run([
      { type: 'question', id: 'q1', ask: ASK },
      { type: 'question-resolved', id: 'q1' },
      { type: 'finished', ok: false, deniedTools: [], interrupted: true },
      { type: 'exited', code: 0 },
    ]);
    const notices = state.items.filter((m) => m.kind === 'notice');
    expect(notices).toEqual([{ id: 't1:1', kind: 'notice', tone: 'info', text: 'Stopped.', at: 1002, deniedTools: undefined }]);
    expect(isOver(state)).toBe(true);
  });

  test('told more while it works, the turn runs on past its first end', () => {
    let state = run([{ type: 'text', text: 'one' }]);
    state = sayMore(state, 'and two?', 2000);
    expect(state.items[1]).toEqual({ id: 't1:1', kind: 'user', text: 'and two?', at: 2000 });
    state = run([FINISHED], state);
    expect(state.status).toBe('finished');
    state = run([{ type: 'text', text: 'two' }], state);
    expect(state.status).toBe('running');
    state = run([FINISHED, { type: 'exited', code: 0 }], state);
    expect(state.status).toBe('ended');
    expect(state.items.map((m) => m.kind)).toEqual(['assistant', 'user', 'assistant']);
  });

  test('the mode the agent reports is kept', () => {
    expect(run([{ type: 'mode-changed', mode: 'plan' }]).mode).toBe('plan');
  });
});

describe('what was answered, in a line', () => {
  test('each sort of answer', () => {
    expect(answerLine(ASK, { kind: 'permission', choiceId: 'once' })).toBe('Allow once');
    expect(answerLine(ASK, { kind: 'permission', choiceId: 'deny', reason: ' not here ' })).toBe('Deny: not here');
    const choices: QuestionKind = {
      kind: 'choices',
      questions: [
        { id: 'a', question: 'Color?', options: [{ label: 'Red' }], multiSelect: true, allowOther: true },
        { id: 'b', question: 'Size?', options: [{ label: 'S' }], multiSelect: false, allowOther: true },
      ],
    };
    expect(
      answerLine(choices, {
        kind: 'choices',
        answers: [
          { questionId: 'a', labels: ['Red', 'Blue'], other: 'teal' },
          { questionId: 'b', labels: ['S'] },
        ],
      }),
    ).toBe('Red, Blue, teal · S');
    const plan: QuestionKind = { kind: 'plan', plan: '1. do it' };
    expect(answerLine(plan, { kind: 'plan', approve: true })).toBe('Approved');
    expect(answerLine(plan, { kind: 'plan', approve: false, feedback: 'smaller' })).toBe('Keep planning: smaller');
    expect(answerLine(plan, { kind: 'permission', choiceId: 'deny' })).toBe('Declined');
  });
});

describe('options', () => {
  const offered: AgentOptions = {
    modes: [
      { id: 'plan', label: 'Plan', dangerous: false },
      { id: 'bypassPermissions', label: 'Bypass permissions', dangerous: true },
    ],
    models: [
      { id: 'default', label: 'Default', efforts: ['low', 'high'] },
      { id: 'small', label: 'Small', efforts: [] },
    ],
    questions: true,
    followUps: true,
    liveChanges: true,
    assumed: false,
  };

  test('what the agent does not offer is not asked for', () => {
    expect(fitOptions(offered, { mode: 'acceptEdits', model: 'gone', effort: 'max' })).toEqual({
      mode: undefined,
      model: undefined,
      effort: undefined,
    });
    expect(fitOptions(offered, { mode: 'plan', model: 'small', effort: 'high' })).toEqual({
      mode: 'plan',
      model: 'small',
      effort: undefined,
    });
    // No model chosen: the efforts are those of the agent's default one.
    expect(fitOptions(offered, { effort: 'high' })).toEqual({ mode: undefined, model: undefined, effort: 'high' });
    // Before the agent said anything, nothing is taken out.
    expect(fitOptions(undefined, { mode: 'x', model: 'y', effort: 'z' })).toEqual({ mode: 'x', model: 'y', effort: 'z' });
  });

  test('they read as a line', () => {
    expect(optionsLine(offered, {})).toBe('Default');
    expect(optionsLine(offered, { mode: 'plan', model: 'small', effort: 'high' })).toBe('Plan · Small · high');
    expect(optionsLine(undefined, { mode: 'auto' })).toBe('auto');
  });
});

describe('what the agent thinks', () => {
  test('is a note of its own, one row until something else comes', () => {
    let state = initialTurn('t');
    for (const event of [
      { type: 'thought', text: 'The file exists. ' },
      { type: 'thought', text: 'I only reply.' },
      { type: 'text-delta', text: 'done' },
      { type: 'thought', text: 'Again.' },
    ] as TurnEvent[]) {
      state = reduceTurn(state, event, 1);
    }
    expect(state.items.map((m) => [m.kind, 'text' in m ? m.text : ''])).toEqual([
      ['note', 'The file exists. I only reply.'],
      ['assistant', 'done'],
      ['note', 'Again.'],
    ]);
    expect(state.items[0]).toMatchObject({ kind: 'note', label: 'thinking' });
    // The answer after the thought is settled by it, not continued.
    expect(state.items[1]).toMatchObject({ streaming: false });
  });
});

describe('options from before the agents were driven as sessions', () => {
  const offered: AgentOptions = {
    modes: [{ id: 'plan', label: 'Plan', dangerous: false }],
    models: [{ id: 'small', label: 'Small', efforts: ['low'] }],
    questions: true,
    followUps: false,
    liveChanges: true,
    assumed: false,
  };

  test('a mode no agent has is dropped whatever is known of the agent', () => {
    for (const mode of ['default', 'edit', 'accept-edits', 'bypass']) {
      expect(withoutLegacyMode({ mode, model: 'small' })).toEqual({ mode: undefined, model: 'small' });
    }
    expect(withoutLegacyMode({ mode: 'plan' })).toEqual({ mode: 'plan' });
    expect(withoutLegacyMode(undefined)).toEqual({});
  });

  test('what is stored is fitted to what the agent offers, and only what changed', () => {
    const changed = refitStored(offered, {
      old: { mode: 'bypass', model: 'small', effort: 'low' },
      gone: { mode: 'plan', model: 'removed', effort: 'low' },
      fine: { mode: 'plan', model: 'small', effort: 'low' },
      none: undefined,
    });
    expect(changed).toEqual({
      old: { mode: undefined, model: 'small', effort: 'low' },
      gone: { mode: 'plan', model: undefined, effort: 'low' },
    });
    // Before the agent said what it offers only the made-up modes go.
    expect(refitStored(undefined, { old: { mode: 'edit', model: 'x' }, fine: { mode: 'm' } })).toEqual({
      old: { mode: undefined, model: 'x', effort: undefined },
    });
  });

  test('the agent\'s list of todos is one checklist row that each new list replaces', () => {
    const list = (detail: string, summary: string): TurnEvent => ({
      type: 'tool-use',
      id: 'leon:todos',
      name: 'TodoWrite',
      summary,
      detail,
    });
    const state = run([
      list('[x] Read\n[~] Write\n[ ] Ship', '1 of 3 done'),
      { type: 'text', text: 'Working.' },
      list('[x] Read\n[x] Write\n[~] Ship', '2 of 3 done'),
    ]);
    const rows = state.items.filter((m) => m.kind === 'todos');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      kind: 'todos',
      todos: [
        { text: 'Read', status: 'completed' },
        { text: 'Write', status: 'completed' },
        { text: 'Ship', status: 'in-progress' },
      ],
    });
    // It never shows as a tool, and "stopped" does not apply to it.
    expect(state.items.some((m) => m.kind === 'tool')).toBe(false);
    expect(run([FINISHED, { type: 'exited', code: 0 }], state).items.filter((m) => m.kind === 'todos')).toHaveLength(1);
  });

  test('a list emptied by the agent leaves no row', () => {
    const state = run([
      { type: 'tool-use', id: 'leon:todos', name: 'TodoWrite', summary: '1 of 1 done', detail: '[x] A' },
      { type: 'tool-use', id: 'leon:todos', name: 'TodoWrite', summary: '', detail: '' },
    ]);
    expect(state.items).toEqual([]);
  });

  test('parseTodos reads the lines the host writes and skips the rest', () => {
    expect(parseTodos('[x] a\n[~] b\n[ ] c\nnoise\n')).toEqual([
      { text: 'a', status: 'completed' },
      { text: 'b', status: 'in-progress' },
      { text: 'c', status: 'pending' },
    ]);
  });
});
