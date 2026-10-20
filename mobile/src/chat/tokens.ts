// The tokens of a turn in a few words, for the foot of a chat, as the desktop
// says them: `48.2k tokens, 62 tok/s`. Pure.
import type { TurnTokens } from '@/link/types';

/** A count of tokens, short: `950`, `12.3k`, `1.2M`. */
export function tokenCount(tokens: number): string {
  if (tokens < 1_000) return String(tokens);
  if (tokens < 1_000_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}

/**
 * What the turn read and wrote, with how fast it wrote when its length is
 * known: `48.2k tokens, 62 tok/s`. Nothing when it used none.
 */
export function tokensLine(tokens: TurnTokens | undefined, durationMs?: number): string | undefined {
  if (!tokens) return undefined;
  const total = tokens.input + tokens.output;
  if (total <= 0) return undefined;
  const line = `${tokenCount(total)} tokens`;
  if (!durationMs || durationMs <= 0 || tokens.output <= 0) return line;
  return `${line}, ${((tokens.output * 1000) / durationMs).toFixed(0)} tok/s`;
}
