// One of King's workers: where it is and what it runs on, and its
// conversation with King, read as it goes on. What it asks is answered here;
// it can be told more (unless it runs in a terminal of the desktop window,
// where only the person there can), interrupted in a turn, or stopped.
import Feather from '@expo/vector-icons/Feather';
import { FlashList } from '@shopify/flash-list';
import { useLocalSearchParams } from 'expo-router';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { ActivityIndicator, Alert, Pressable, StyleSheet, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import {
  answerRequest,
  dollars,
  interruptRequest,
  machineLine,
  piecesOf,
  runsOn,
  stopWorkerRequest,
  tellWorkerRequest,
  workerActions,
  workerWord,
  type KingReply,
  type Piece,
} from '@/king/model';
import { tap } from '@/store/alerts';
import { useApp } from '@/store/app';
import { tryKing, useKing, watchWorker, workerKey } from '@/store/king';
import { ensureLink, useRuntime } from '@/store/runtime';
import { agentColor, fonts, metrics } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';
import { ConnectionBanner } from '@/ui/ConnectionBanner';
import { KingPiece, moodColor, Working } from '@/ui/King';
import { KingQuestionPanel } from '@/ui/KingQuestion';
import { Button, Callout, Label, Led, Rule, Screen, Txt } from '@/ui/kit';

export default function Worker() {
  const params = useLocalSearchParams<{ hostId: string; worker: string }>();
  const hostId = params.hostId;
  const id = Number(params.worker);
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const host = useApp((s) => s.hosts.find((h) => h.hostId === hostId));
  const online = useRuntime((s) => s.conn[hostId]?.kind === 'online');
  const worker = useKing((s) => s.status[hostId]?.workers.find((w) => w.id === id));
  const entries = useKing((s) => s.workers[workerKey(hostId, id)]);
  const error = useKing((s) => s.error[hostId]);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);

  useEffect(() => {
    ensureLink(hostId);
  }, [hostId]);
  useEffect(() => watchWorker(hostId, id), [hostId, id]);

  const pieces = useMemo(() => piecesOf(entries ?? []), [entries]);
  const agent = worker?.conversation.agent ?? '';
  const renderItem = useCallback(
    ({ item }: { item: Piece }) => (
      <KingPiece piece={item} who={`Worker #${id}`} asker="King" agent={agent} cwd={worker?.cwd} />
    ),
    [id, agent, worker?.cwd],
  );
  const answer = useCallback(
    (question: string, reply: KingReply, words: string) =>
      tryKing(hostId, answerRequest({ worker: id }, question, reply, words)),
    [hostId, id],
  );
  const tell = useCallback(() => {
    const said = draft.trim();
    if (!said) return;
    tap();
    setSending(true);
    void tryKing(hostId, tellWorkerRequest(id, said)).then((done) => {
      setSending(false);
      if (done) setDraft('');
    });
  }, [draft, hostId, id]);

  if (!host) {
    return (
      <Screen title="Not found">
        <View style={{ padding: 16 }}>
          <Txt tone="muted">This computer is no longer connected to this device.</Txt>
        </View>
      </Screen>
    );
  }

  const { mood, word } = workerWord(worker?.state ?? '');
  const color = moodColor(t, mood);
  const actions = worker ? workerActions(worker) : { tell: false, interrupt: false, stop: false };
  const canSend = online && !sending && draft.trim().length > 0;
  const stop = () =>
    Alert.alert(
      `Stop worker #${id}?`,
      'Its turn ends now and what waited for it is dropped. King hears it was stopped; it can be told more later.',
      [
        { text: 'Keep it', style: 'cancel' },
        { text: 'Stop', style: 'destructive', onPress: () => void tryKing(hostId, stopWorkerRequest(id)) },
      ],
    );

  return (
    <Screen kind={`Worker #${id}`} title={worker?.title ?? `Worker #${id}`} subtitle={worker?.cwd} bottomInset={false}>
      <ConnectionBanner hostId={hostId} />
      {worker ? (
        <View style={{ paddingHorizontal: 16, paddingVertical: 10, gap: 4, borderBottomWidth: 1, borderBottomColor: t.border }}>
          <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
            <Led color={color} hollow={mood === 'off'} />
            <Label style={{ color }}>{word}</Label>
            {worker.conversation.costUsd > 0 ? (
              <Label tone="faint" style={{ marginLeft: 'auto' }}>
                {dollars(worker.conversation.costUsd)}
              </Label>
            ) : null}
          </View>
          <Label numberOfLines={1} style={{ color: agentColor(t, agent), textTransform: 'none' }}>
            {runsOn(worker.conversation)}
            {worker.project ? ` · ${worker.project}` : ''}
            {worker.machine ? ` · ${machineLine(worker)}` : ''}
          </Label>
          {worker.terminal ? (
            <Label tone="faint" style={{ textTransform: 'none', letterSpacing: 0 }}>
              In a terminal of Leon&apos;s window: only the person there can tell it more.
            </Label>
          ) : null}
        </View>
      ) : null}
      {error ? (
        <Callout tone="error" style={{ marginHorizontal: 16, marginTop: 8 }}>
          <Txt variant="small">{error}</Txt>
        </Callout>
      ) : null}
      <KeyboardAvoidingView behavior="padding" keyboardVerticalOffset={-insets.bottom} style={{ flex: 1 }}>
        <View style={{ flex: 1 }}>
          <FlashList
            data={pieces}
            renderItem={renderItem}
            keyExtractor={(piece) => piece.key}
            getItemType={(piece) => (piece.kind === 'tools' ? 'tools' : piece.entry.kind)}
            keyboardDismissMode="interactive"
            keyboardShouldPersistTaps="handled"
            maintainVisibleContentPosition={{
              autoscrollToBottomThreshold: 0.25,
              startRenderingFromBottom: true,
            }}
            contentContainerStyle={{ paddingTop: 4, paddingBottom: 16 }}
            ListEmptyComponent={
              entries === undefined && online && !error ? (
                <View style={{ padding: 24 }}>
                  <ActivityIndicator color={t.textMuted} />
                </View>
              ) : (
                <View style={{ padding: 24 }}>
                  <Txt tone="muted">Nothing said yet.</Txt>
                </View>
              )
            }
            ListFooterComponent={
              worker?.conversation.busy ? (
                <Working word={`Worker #${id} is working`} color={agentColor(t, agent)} since={worker.conversation.busySinceMs} />
              ) : null
            }
          />
        </View>
        {worker ? (
          <View style={{ backgroundColor: t.surface, paddingBottom: insets.bottom + (actions.tell ? 0 : 10) }}>
            <Rule />
            {worker.conversation.questions.length > 0 ? (
              <KingQuestionPanel
                key={worker.conversation.questions[0]?.id}
                questions={worker.conversation.questions}
                who={`Worker #${id}`}
                online={online}
                onAnswer={answer}
              />
            ) : null}
            {actions.interrupt || actions.stop ? (
              <View style={styles.actions}>
                {actions.interrupt ? (
                  <Button
                    compact
                    icon="pause"
                    title="Interrupt"
                    disabled={!online}
                    accessibilityHint="Ends its turn now; it keeps its conversation."
                    onPress={() => void tryKing(hostId, interruptRequest({ worker: id }))}
                  />
                ) : null}
                {actions.stop ? (
                  <Button compact kind="danger" icon="square" title="Stop" disabled={!online} onPress={stop} />
                ) : null}
              </View>
            ) : null}
            {actions.tell ? (
              <View style={styles.inputRow}>
                <Txt variant="mono" tone={online ? 'signal' : 'faint'} style={styles.prompt}>
                  {'>'}
                </Txt>
                <TextInput
                  accessibilityLabel={`Message to worker ${id}`}
                  value={draft}
                  onChangeText={setDraft}
                  placeholder={online ? 'Tell it more' : 'Not connected'}
                  placeholderTextColor={t.textFaint}
                  selectionColor={t.signal}
                  cursorColor={t.signal}
                  multiline
                  editable={!sending}
                  style={[styles.input, { color: t.text }]}
                />
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel="Send"
                  accessibilityState={{ disabled: !canSend, busy: sending }}
                  disabled={!canSend}
                  onPress={tell}
                  style={({ pressed }) => [
                    styles.send,
                    canSend
                      ? { backgroundColor: t.accentFill, opacity: pressed ? 0.85 : 1 }
                      : { backgroundColor: t.surface2, borderWidth: 1, borderColor: t.border },
                  ]}>
                  {sending ? (
                    <ActivityIndicator color={t.textMuted} />
                  ) : (
                    <Feather name="arrow-up" size={20} color={canSend ? t.onAccentFill : t.textFaint} />
                  )}
                </Pressable>
              </View>
            ) : null}
          </View>
        ) : null}
      </KeyboardAvoidingView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  actions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
    paddingHorizontal: metrics.pad,
    paddingTop: 10,
  },
  inputRow: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
    paddingLeft: metrics.pad,
    paddingRight: 8,
    paddingVertical: 8,
  },
  prompt: { fontSize: 16, lineHeight: 22, paddingBottom: 13 },
  input: {
    flex: 1,
    minHeight: 48,
    maxHeight: 160,
    paddingHorizontal: 0,
    paddingTop: 13,
    paddingBottom: 13,
    fontFamily: fonts.sans,
    fontSize: metrics.textBody,
    lineHeight: 22,
  },
  send: {
    width: 48,
    height: 48,
    borderRadius: metrics.radius,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
