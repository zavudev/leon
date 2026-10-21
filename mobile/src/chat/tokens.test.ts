import { describe, expect, test } from 'bun:test';

import { reduceTurn, initialTurn } from './reduce';
import { tokenCount, tokensLine } from './tokens';

const TOKENS = { input: 45_100, cachedInput: 31_000, output: 3_100, reasoning: 0 };

describe('the tokens of a turn', () => {
  test('a count is short', () => {
    expect(tokenCount(950)).toBe('950');
    expect(tokenCount(12_345)).toBe('12.3k');
    expect(tokenCount(1_234_567)).toBe('1.2M');
  });

  test('a line says the tokens, and how fast they were written when the length is known', () => {
    expect(tokensLine(TOKENS, 50_000)).toBe('48.2k tokens, 62 tok/s');
    expect(tokensLine(TOKENS)).toBe('48.2k tokens');
    expect(tokensLine({ ...TOKENS, output: 0 }, 50_000)).toBe('45.1k tokens');
    expect(tokensLine({ input: 0, cachedInput: 0, output: 0, reasoning: 0 }, 1)).toBeUndefined();
    expect(tokensLine(undefined, 1)).toBeUndefined();
  });

  test('each count of a turn replaces the one before', () => {
    let state = initialTurn('t');
    state = reduceTurn(state, { type: 'usage', tokens: { ...TOKENS, output: 10 } }, 0);
    state = reduceTurn(state, { type: 'usage', tokens: TOKENS }, 0);
    expect(state.tokens).toEqual(TOKENS);
    expect(state.status).toBe('running');
  });
});
