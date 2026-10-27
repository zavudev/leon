// The parts of King's screens: a piece of a conversation (what was asked,
// what was answered, the tools used, news and notes), the card of a worker,
// the line that says King works, and a run with what can be done to it.
import Feather from '@expo/vector-icons/Feather';
import { memo, useMemo, useState } from 'react';
import { Alert, Pressable, StyleSheet, TextInput, View } from 'react-native';
import Markdown from 'react-native-markdown-display';

import { fromCwd } from '@/chat/model';
import {
  dollars,
  firstLine,
  machineLine,
  runActions,
  runLine,
  runsOn,
  workerWord,
  type KingRun,
  type KingWorker,
  type Piece,
  type WorkerMood,
} from '@/king/model';
import { agentColor, fonts, metrics, type Palette } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';

import { useClock } from './clock';
import { Button, Callout, Label, Led, Txt } from './kit';
import { markdownParser, markdownRules, markdownStyles, openLink } from './MessageRow';

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
          <Markdown style={md} onLinkPress={openLink} markdownit={markdownParser} rules={markdownRules}>
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
            {worker.machine ? ` · ${machineLine(worker)}` : ''}
            {worker.terminal ? ' · in a terminal' : ''}
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

/** A field of one line, in the style of the run's own controls. */
function Line({
  label,
  value,
  onChangeText,
  placeholder,
  mono = false,
}: {
  label: string;
  value: string;
  onChangeText(text: string): void;
  placeholder: string;
  mono?: boolean;
}) {
  const t = useTheme();
  return (
    <TextInput
      accessibilityLabel={label}
      value={value}
      onChangeText={onChangeText}
      placeholder={placeholder}
      placeholderTextColor={t.textFaint}
      selectionColor={t.signal}
      cursorColor={t.signal}
      autoCorrect={!mono}
      autoCapitalize={mono ? 'none' : 'sentences'}
      style={[
        styles.line,
        { color: t.text, borderColor: t.border, backgroundColor: t.surface, fontFamily: mono ? fonts.mono : fonts.sans },
      ]}
    />
  );
}

/**
 * A run of a task's steps: where it is, and while it is not over, what can be
 * done to it. A blocked one goes on, with words for the workers of its failed
 * steps; one not over is called off, once the person says so.
 */
export function RunRow({
  run,
  online,
  onContinue,
  onCancel,
}: {
  run: KingRun;
  online: boolean;
  onContinue(words: string): Promise<boolean>;
  onCancel(): void;
}) {
  const actions = runActions(run);
  const [open, setOpen] = useState(false);
  const [words, setWords] = useState('');
  const [sending, setSending] = useState(false);
  return (
    <View style={{ gap: 4 }}>
      <Label tone={run.state === 'blocked' ? 'error' : run.state === 'done' ? 'success' : 'muted'}>{runLine(run)}</Label>
      <Txt variant="small" numberOfLines={1}>
        {run.title}
        {run.project ? ` · ${run.project}` : ''}
      </Txt>
      {run.note ? (
        <Txt variant="small" tone="error" numberOfLines={2}>
          {run.note}
        </Txt>
      ) : null}
      {open && actions.continue ? (
        <View style={{ gap: 6, marginTop: 2 }}>
          <Line
            label={`What to tell the workers of run ${run.id}`}
            value={words}
            onChangeText={setWords}
            placeholder="What to tell its failed steps (optional)"
          />
          <View style={styles.actions}>
            <Button
              compact
              title="Go on"
              busy={sending}
              disabled={!online}
              onPress={() => {
                setSending(true);
                void onContinue(words).then((done) => {
                  setSending(false);
                  if (done) {
                    setOpen(false);
                    setWords('');
                  }
                });
              }}
            />
            <Button compact title="Not now" onPress={() => setOpen(false)} />
          </View>
        </View>
      ) : actions.continue || actions.cancel ? (
        <View style={[styles.actions, { marginTop: 2 }]}>
          {actions.continue ? (
            <Button compact icon="play" title="Continue" disabled={!online} onPress={() => setOpen(true)} />
          ) : null}
          {actions.cancel ? (
            <Button
              compact
              kind="danger"
              title="Cancel"
              disabled={!online}
              onPress={() =>
                Alert.alert(
                  `Cancel run ${run.id}?`,
                  'Its workers are stopped and no step starts again. What its steps committed stays.',
                  [
                    { text: 'Keep it', style: 'cancel' },
                    { text: 'Cancel the run', style: 'destructive', onPress: onCancel },
                  ],
                )
              }
            />
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

/** A field for a task of the board by its id, and what starts a run of it. */
export function StartRun({ online, onStart }: { online: boolean; onStart(task: string): Promise<boolean> }) {
  const [task, setTask] = useState('');
  const [sending, setSending] = useState(false);
  return (
    <View style={[styles.actions, { alignItems: 'center' }]}>
      <View style={{ flex: 1 }}>
        <Line label="Task to run, by its id" value={task} onChangeText={setTask} placeholder="Task id" mono />
      </View>
      <Button
        compact
        title="Start a run"
        busy={sending}
        disabled={!online || task.trim().length === 0}
        onPress={() => {
          setSending(true);
          void onStart(task).then((done) => {
            setSending(false);
            if (done) setTask('');
          });
        }}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
  line: {
    minHeight: metrics.touch,
    paddingHorizontal: 10,
    paddingVertical: 9,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radius,
    fontSize: metrics.textSmall,
    lineHeight: 19,
  },
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
