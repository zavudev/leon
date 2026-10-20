// The parts of King's screens: a piece of a conversation (what was asked,
// what was answered, the tools used, news and notes), the card of a worker,
// and the line that says King works.
import Feather from '@expo/vector-icons/Feather';
import { memo, useMemo, useState } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import Markdown from 'react-native-markdown-display';

import { fromCwd } from '@/chat/model';
import {
  dollars,
  firstLine,
  runsOn,
  workerWord,
  type KingWorker,
  type Piece,
  type WorkerMood,
} from '@/king/model';
import { agentColor, metrics, type Palette } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';

import { useClock } from './clock';
import { Callout, Label, Led, Txt } from './kit';
import { markdownStyles, openLink } from './MessageRow';

export function moodColor(t: Palette, mood: WorkerMood): string {
  if (mood === 'asking') return t.warning;
  if (mood === 'failed') return t.error;
  if (mood === 'done') return t.success;
  return mood === 'working' ? t.signal : t.textFaint;
}

/** A line that folds what it holds: the tools of a turn, Leon's news, a report. */
function Folded({
  head,
  lines,
  cwd,
}: {
  head: string;
  lines: string[];
  cwd?: string;
}) {
  const t = useTheme();
  const [open, setOpen] = useState(false);
  const more = lines.some((line) => line.trim());
  return (
    <View style={[styles.folded, { borderLeftColor: open ? t.gridMark : t.guide }]}>
      <Pressable
        accessibilityRole="button"
        accessibilityState={more ? { expanded: open } : undefined}
        accessibilityLabel={head}
        accessibilityHint={more ? (open ? 'Hides it.' : 'Shows it.') : undefined}
        disabled={!more}
        onPress={() => setOpen((was) => !was)}
        style={({ pressed }) => [styles.foldedHead, { backgroundColor: pressed ? t.surface2 : 'transparent' }]}>
        <Txt variant="mono" tone="faint" numberOfLines={1} style={{ flex: 1 }}>
          {head}
        </Txt>
        {more ? (
          <Txt variant="mono" tone="faint" style={{ width: 14, textAlign: 'right' }}>
            {open ? '▾' : '▸'}
          </Txt>
        ) : null}
      </Pressable>
      {open && more ? (
        <View style={{ gap: 4, paddingBottom: 8 }}>
          {lines.map((line, at) => (
            <Txt key={at} variant="mono" tone="muted" selectable>
              {fromCwd(line, cwd)}
            </Txt>
          ))}
        </View>
      ) : null}
    </View>
  );
}

/**
 * One piece of a conversation of King's. `who` names the agent that answers
 * (King, or a worker), `agent` its tag for its colour, `asker` who prompts it
 * (the person, or King); `cwd` is the folder its paths are said from.
 */
function KingPieceInner({
  piece,
  who,
  agent,
  asker,
  cwd,
}: {
  piece: Piece;
  who: string;
  agent: string;
  asker: string;
  cwd?: string;
}) {
  const t = useTheme();
  const md = useMemo(() => markdownStyles(t), [t]);
  if (piece.kind === 'tools') {
    const { entries } = piece;
    const head =
      entries.length === 1
        ? firstLine(entries[0]!.text)
        : `${entries.length} tools · ${firstLine(entries.at(-1)!.text)}`;
    return <Folded head={head} lines={entries.map((e) => e.text)} cwd={cwd} />;
  }
  const { entry } = piece;
  switch (entry.kind) {
    case 'prompt':
      return (
        <View style={[styles.prompt, { backgroundColor: t.surface, borderColor: t.border, borderLeftColor: t.signal }]}>
          <View style={styles.who}>
            <Txt variant="mono" tone="signal">
              {'>'}
            </Txt>
            <Label tone="muted">{asker}</Label>
          </View>
          <Txt selectable>{entry.text}</Txt>
        </View>
      );
    case 'agent':
      return (
        <View style={styles.agent}>
          <View style={styles.who}>
            <Led color={agentColor(t, agent)} />
            <Label style={{ color: agentColor(t, agent) }}>{who}</Label>
          </View>
          <Markdown style={md} onLinkPress={openLink}>
            {entry.text}
          </Markdown>
        </View>
      );
    case 'news': {
      const lines = entry.text.replace(/^\[Leon\][^\n]*\n*/, '').split('\n').filter((line) => line.trim());
      return <Folded head={`News of the workers · ${lines.length}`} lines={lines} cwd={cwd} />;
    }
    case 'report':
      return <Folded head={firstLine(entry.text)} lines={entry.text.split('\n').slice(1)} />;
    case 'question':
      return (
        <Callout tone="warning" style={styles.callout}>
          <Label tone="warning">{who} asks</Label>
          <Txt selectable>{entry.text}</Txt>
        </Callout>
      );
    case 'notice':
      return (
        <Callout tone="info" style={styles.callout}>
          <Txt variant="small" tone="muted" selectable>
            {entry.text}
          </Txt>
        </Callout>
      );
  }
}

export const KingPiece = memo(KingPieceInner);

/** What says that King or a worker is in a turn, and since when. */
export function Working({ word, color, since }: { word: string; color: string; since?: number }) {
  const now = useClock();
  const minutes = since ? Math.max(0, Math.floor((now - since) / 60_000)) : undefined;
  return (
    <View style={styles.working}>
      <Led color={color} />
      <Label style={{ color, flex: 1 }} accessibilityLiveRegion="polite">
        {word}
      </Label>
      {minutes !== undefined ? <Label tone="faint">{minutes < 1 ? 'just now' : `${minutes} min`}</Label> : null}
    </View>
  );
}

/** A worker of King's: its number and state, what it is for, where, on what, and its last answer. */
export function WorkerCard({ worker, onPress }: { worker: KingWorker; onPress(): void }) {
  const t = useTheme();
  const { mood, word } = workerWord(worker.state);
  const color = moodColor(t, mood);
  const answer = worker.lastAnswer ? fromCwd(worker.lastAnswer, worker.cwd) : '';
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`Worker ${worker.id}, ${worker.title}, ${word}`}
      accessibilityHint="Opens its conversation."
      onPress={onPress}
      style={({ pressed }) => [
        styles.card,
        {
          backgroundColor: pressed ? t.surface2 : t.surface,
          borderColor: mood === 'asking' ? t.warning : t.border,
          borderLeftColor: color,
        },
      ]}>
      <View style={{ flex: 1, gap: 4 }}>
        <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <Label tone="muted">#{worker.id}</Label>
          <Led color={color} hollow={mood === 'off'} />
          <Label style={{ color }}>{word}</Label>
          {worker.conversation.costUsd > 0 ? (
            <Label tone="faint" style={{ marginLeft: 'auto' }}>
              {dollars(worker.conversation.costUsd)}
            </Label>
          ) : null}
        </View>
        <Txt variant="strong" numberOfLines={2}>
          {worker.title}
        </Txt>
        <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <Led color={agentColor(t, worker.conversation.agent)} />
          <Label numberOfLines={1} style={{ color: agentColor(t, worker.conversation.agent), flexShrink: 1, textTransform: 'none' }}>
            {runsOn(worker.conversation)}
            {worker.project ? ` · ${worker.project}` : ''}
          </Label>
        </View>
        {answer ? (
          <Txt variant="small" tone="muted" numberOfLines={3}>
            {answer}
          </Txt>
        ) : null}
      </View>
      <Feather name="chevron-right" size={18} color={t.textMuted} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  prompt: {
    marginHorizontal: metrics.pad,
    marginTop: 18,
    marginBottom: 12,
    paddingHorizontal: 12,
    paddingVertical: 10,
    gap: 4,
    borderWidth: metrics.hairline,
    borderLeftWidth: 2,
    borderRadius: metrics.radiusCell,
  },
  who: { flexDirection: 'row', alignItems: 'center', gap: 6, marginBottom: 4 },
  agent: { paddingHorizontal: metrics.pad, paddingTop: 4 },
  folded: { marginHorizontal: metrics.pad, marginVertical: 2, paddingLeft: 10, borderLeftWidth: 1 },
  foldedHead: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 32, paddingRight: 4 },
  callout: { marginHorizontal: metrics.pad, marginVertical: 8 },
  working: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginHorizontal: metrics.pad,
    marginTop: 14,
    minHeight: 24,
  },
  card: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    marginHorizontal: 12,
    marginBottom: 6,
    paddingVertical: 10,
    paddingLeft: 12,
    paddingRight: 10,
    borderWidth: 1,
    borderLeftWidth: 2,
  },
});
