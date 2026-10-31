// King of one computer: its conversation with the person, newest at the
// bottom, what it asks, a line to say something to it, and its workers and
// runs, which go on, are called off or are started here. It is followed
// while this screen is open; a worker opens its own conversation.
import Feather from '@expo/vector-icons/Feather';
import { FlashList } from '@shopify/flash-list';
import { router, useLocalSearchParams } from 'expo-router';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import {
  answerRequest,
  piecesOf,
  runCancelRequest,
  runContinueRequest,
  runsOn,
  runStartRequest,
  spentLine,
  workersInOrder,
  type KingReply,
  type Piece,
} from '@/king/model';
import { kingInFront, tap } from '@/store/alerts';
import { useApp } from '@/store/app';
import { kingRefusal, sayToKing, tryKing, useKing, watchKing } from '@/store/king';
import { ensureLink, useRuntime } from '@/store/runtime';
import { agentColor, fonts, metrics } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';
import { ConnectionBanner } from '@/ui/ConnectionBanner';
import { KingPiece, RunRow, StartRun, WorkerCard, Working } from '@/ui/King';
import { KingQuestionPanel } from '@/ui/KingQuestion';
import { Callout, Chip, Label, Rule, Screen, Txt } from '@/ui/kit';

type Tab = 'talk' | 'workers';

export default function King() {
  const { hostId } = useLocalSearchParams<{ hostId: string }>();
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const host = useApp((s) => s.hosts.find((h) => h.hostId === hostId));
  const online = useRuntime((s) => s.conn[hostId]?.kind === 'online');
  const shared = useRuntime((s) => s.shared[hostId]);
  const status = useKing((s) => s.status[hostId]);
  const entries = useKing((s) => s.brain[hostId]?.entries);
  const error = useKing((s) => s.error[hostId]);
  const [tab, setTab] = useState<Tab>('talk');
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);

  useEffect(() => {
    ensureLink(hostId);
  }, [hostId]);
  // Followed for as long as the screen is open, a worker's on top of it included.
  useEffect(() => watchKing(hostId), [hostId]);
  useEffect(() => kingInFront(hostId), [hostId]);

  const pieces = useMemo(() => piecesOf(entries ?? []), [entries]);
  const workers = useMemo(() => workersInOrder(status?.workers ?? []), [status]);
  const runs = status?.runs ?? [];
  const brain = status?.brain;
  const busy = brain?.busy === true;
  const noKing = shared !== undefined && !shared.king;
  const asks = (brain?.questions.length ?? 0) > 0;

  const answer = useCallback(
    (question: string, reply: KingReply, words: string) =>
      tryKing(hostId, answerRequest('brain', question, reply, words)),
    [hostId],
  );
  const startRun = useCallback(
    async (task: string) => {
      const request = runStartRequest(task);
      return request ? tryKing(hostId, request) : false;
    },
    [hostId],
  );

  const canSend = online && !sending && !noKing && draft.trim().length > 0;
  const submit = useCallback(() => {
    const text = draft.trim();
    if (!text) return;
    tap();
    setSending(true);
    setRefused(null);
    sayToKing(hostId, text)
      .then(() => setDraft(''))
      .catch((failure: unknown) => setRefused(kingRefusal(failure)))
      .finally(() => setSending(false));
  }, [draft, hostId]);

  const renderItem = useCallback(
    ({ item }: { item: Piece }) => <KingPiece piece={item} who="Orchestrator" asker="You" agent={brain?.agent ?? ''} />,
    [brain?.agent],
  );

  if (!host) {
    return (
      <Screen title="Not found">
        <View style={{ padding: 16 }}>
          <Txt tone="muted">This computer is no longer connected to this device.</Txt>
        </View>
      </Screen>
    );
  }

  return (
    <Screen
      kind="Orchestrator"
      title={host.hostName}
      subtitle={status ? `${runsOn(status.brain)} · ${spentLine(status)}` : undefined}
      bottomInset={false}>
      <ConnectionBanner
        hostId={hostId}
        detail={busy ? 'Orchestrator is working' : asks ? 'Orchestrator asks you' : undefined}
        detailLive={busy}
        detailWaits={asks}
      />
      <View style={[styles.tabs, { borderBottomColor: t.border }]}>
        <Chip title="Talk" selected={tab === 'talk'} onPress={() => setTab('talk')} />
        <Chip
          title={`Workers · ${workers.length}`}
          selected={tab === 'workers'}
          onPress={() => setTab('workers')}
        />
        {runs.length > 0 ? (
          <Label tone="faint" style={{ marginLeft: 'auto' }}>
            {runs.length} {runs.length === 1 ? 'run' : 'runs'}
          </Label>
        ) : null}
      </View>
      {noKing ? (
        <View style={{ padding: 16, gap: 8 }}>
          <Label tone="faint">[ No Orchestrator ]</Label>
          <Txt tone="muted">
            This computer has no Orchestrator to speak to. It needs a newer Leon there, with Orchestrator set up.
          </Txt>
        </View>
      ) : tab === 'workers' ? (
        <ScrollView
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={{ paddingTop: 12, paddingBottom: insets.bottom + 16 }}>
          {status ? (
            <View style={{ paddingHorizontal: 16, paddingBottom: 16, gap: 12 }}>
              <Label tone="faint">[ Runs ]</Label>
              {runs.map((run) => (
                <RunRow
                  key={run.id}
                  run={run}
                  online={online}
                  onContinue={(words) => tryKing(hostId, runContinueRequest(run.id, words))}
                  onCancel={() => void tryKing(hostId, runCancelRequest(run.id))}
                />
              ))}
              <StartRun online={online} onStart={startRun} />
            </View>
          ) : null}
          <View style={{ paddingHorizontal: 16, paddingBottom: 8 }}>
            <Label tone="faint">[ Workers ]</Label>
          </View>
          {workers.length === 0 ? (
            <View style={{ paddingHorizontal: 16 }}>
              <Txt tone="muted">
                {status ? 'Orchestrator has no workers now. Ask it for something and it starts them.' : ' '}
              </Txt>
            </View>
          ) : (
            workers.map((worker) => (
              <WorkerCard
                key={worker.id}
                worker={worker}
                onPress={() =>
                  router.push({
                    pathname: '/host/[hostId]/worker',
                    params: { hostId, worker: String(worker.id) },
                  })
                }
              />
            ))
          )}
        </ScrollView>
      ) : (
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
                  <View style={{ padding: 24, gap: 8 }}>
                    <Label tone="muted">[ Nothing said yet ]</Label>
                    <Txt variant="strong">Tell Orchestrator what you want done.</Txt>
                    <Txt tone="muted">
                      It plans the work, gives it to workers on that computer, checks what they did,
                      and tells you here.
                    </Txt>
                  </View>
                )
              }
              ListFooterComponent={
                busy ? (
                  <Working word="Orchestrator is working" color={agentColor(t, brain?.agent ?? '')} since={brain?.busySinceMs} />
                ) : null
              }
            />
          </View>
          <Rule />
          <View style={{ backgroundColor: t.surface, paddingBottom: insets.bottom }}>
            {brain && asks ? (
              <KingQuestionPanel
                key={brain.questions[0]?.id}
                questions={brain.questions}
                who="Orchestrator"
                online={online}
                onAnswer={answer}
              />
            ) : null}
            {refused || error ? (
              <Callout tone="error" style={styles.note}>
                <Txt variant="small">{refused ?? error}</Txt>
              </Callout>
            ) : null}
            <View style={styles.inputRow}>
              <Txt variant="mono" tone={online ? 'signal' : 'faint'} style={styles.prompt}>
                {'>'}
              </Txt>
              <TextInput
                accessibilityLabel="Message to Orchestrator"
                value={draft}
                onChangeText={setDraft}
                placeholder={online ? (busy ? 'Tell Orchestrator more' : 'Ask Orchestrator') : 'Not connected'}
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
                onPress={submit}
                style={({ pressed }) => [
                  styles.action,
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
          </View>
        </KeyboardAvoidingView>
      )}
    </Screen>
  );
}

const styles = StyleSheet.create({
  tabs: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: metrics.pad,
    paddingVertical: 8,
    borderBottomWidth: 1,
  },
  note: { marginHorizontal: metrics.pad, marginTop: 8 },
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
  action: {
    width: 48,
    height: 48,
    borderRadius: metrics.radius,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
