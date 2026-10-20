// One of King's workers: where it is and what it runs on, and its
// conversation with King, read as it goes on. Only to read: King tells it
// what to do.
import { FlashList } from '@shopify/flash-list';
import { useLocalSearchParams } from 'expo-router';
import { useCallback, useEffect, useMemo } from 'react';
import { ActivityIndicator, View } from 'react-native';

import { dollars, piecesOf, runsOn, workerWord, type Piece } from '@/king/model';
import { useApp } from '@/store/app';
import { useKing, watchWorker, workerKey } from '@/store/king';
import { ensureLink, useRuntime } from '@/store/runtime';
import { agentColor } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';
import { ConnectionBanner } from '@/ui/ConnectionBanner';
import { KingPiece, moodColor, Working } from '@/ui/King';
import { Callout, Label, Led, Screen, Txt } from '@/ui/kit';

export default function Worker() {
  const params = useLocalSearchParams<{ hostId: string; worker: string }>();
  const hostId = params.hostId;
  const id = Number(params.worker);
  const t = useTheme();
  const host = useApp((s) => s.hosts.find((h) => h.hostId === hostId));
  const online = useRuntime((s) => s.conn[hostId]?.kind === 'online');
  const worker = useKing((s) => s.status[hostId]?.workers.find((w) => w.id === id));
  const entries = useKing((s) => s.workers[workerKey(hostId, id)]);
  const error = useKing((s) => s.error[hostId]);

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

  return (
    <Screen kind={`Worker #${id}`} title={worker?.title ?? `Worker #${id}`} subtitle={worker?.cwd}>
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
          </Label>
        </View>
      ) : null}
      {error ? (
        <Callout tone="error" style={{ marginHorizontal: 16, marginTop: 8 }}>
          <Txt variant="small">{error}</Txt>
        </Callout>
      ) : null}
      <FlashList
        data={pieces}
        renderItem={renderItem}
        keyExtractor={(piece) => piece.key}
        getItemType={(piece) => (piece.kind === 'tools' ? 'tools' : piece.entry.kind)}
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
    </Screen>
  );
}
