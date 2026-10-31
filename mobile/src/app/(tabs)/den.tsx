// The Den: every agent that is live on the paired computers, as a lion, with
// what it is doing as its own computer says it. Who waits for the person is
// first; a touch opens its chat.
import Feather from '@expo/vector-icons/Feather';
import { FlashList } from '@shopify/flash-list';
import { useFocusEffect } from 'expo-router';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { Pressable, View } from 'react-native';

import { lionsOf, tally, type Lion as LionRow, type Mood } from '@/chat/den';
import { buildChatTree, pinnedRows, type ChatRecord, type Worktrees } from '@/chat/list';
import { fromCwd } from '@/chat/model';
import type { SharedState } from '@/link/types';
import { useApp } from '@/store/app';
import { connectAll, useRuntime, watchDens } from '@/store/runtime';
import { agentColor, type Palette } from '@/theme/tokens';
import { useMotion, useTheme } from '@/theme/useTheme';
import { Label, Led, Screen, Txt } from '@/ui/kit';
import { Lion } from '@/ui/Lion';
import { Watermark } from '@/ui/Mark';
import { openRow } from '@/ui/SessionCell';

/** How long a lion holds a frame. */
const BEAT_MS = 420;

function moodColor(t: Palette, mood: Mood): string {
  if (mood === 'waiting') return t.warning;
  if (mood === 'down') return t.error;
  return mood === 'working' ? t.signal : t.textFaint;
}

const GLYPH: Record<Mood, 'help-circle' | 'x-octagon' | 'activity' | 'moon'> = {
  waiting: 'help-circle',
  down: 'x-octagon',
  working: 'activity',
  quiet: 'moon',
};

/** Opens the chat of a lion: the one this device already has for its session, or one made for it. */
function openLion(
  lion: LionRow,
  chats: Record<string, ChatRecord>,
  state: SharedState | undefined,
  trees: Worktrees | undefined,
): void {
  const { cub, hostId } = lion;
  if (!cub.agent || !cub.externalId) return;
  const records = Object.values(chats);
  const tree = buildChatTree(hostId, records, state?.sessions ?? [], state?.projects ?? [], trees);
  const now = Date.now();
  const [found] = pinnedRows(
    hostId,
    records,
    [
      {
        agent: cub.agent,
        externalId: cub.externalId,
        cwd: cub.cwd ?? '',
        title: cub.name,
        startedMs: now,
        updatedMs: now,
        messages: 0,
        canResume: true,
      },
    ],
    tree,
  );
  if (found) openRow(hostId, found.row);
}

export default function Den() {
  const t = useTheme();
  const motion = useMotion();
  const hosts = useApp((s) => s.hosts);
  const chats = useApp((s) => s.chats);
  const dens = useRuntime((s) => s.dens);
  const shared = useRuntime((s) => s.shared);
  const worktrees = useRuntime((s) => s.worktrees);
  const conn = useRuntime((s) => s.conn);

  useEffect(() => {
    connectAll();
  }, []);
  // Asking is what keeps a computer working its Den out: only while in front.
  useFocusEffect(useCallback(() => watchDens(), []));

  const [beat, setBeat] = useState(0);
  const still = motion.open === 0;
  useFocusEffect(
    useCallback(() => {
      if (still) return undefined;
      const timer = setInterval(() => setBeat((at) => at + 1), BEAT_MS);
      return () => clearInterval(timer);
    }, [still]),
  );

  const lions = useMemo(() => lionsOf(hosts, dens), [hosts, dens]);
  const count = tally(lions);

  const open = (lion: LionRow) => openLion(lion, chats, shared[lion.hostId], worktrees[lion.hostId]);

  const online = hosts.filter((host) => conn[host.hostId]?.kind === 'online');
  const withDen = online.filter((host) => shared[host.hostId]?.den);
  const asked = online.every((host) => shared[host.hostId] !== undefined);

  return (
    <Screen kind="Office" title="The Office" back={false} bottomInset={false}>
      <View
        style={{
          flexDirection: 'row',
          alignItems: 'center',
          gap: 14,
          paddingHorizontal: 16,
          paddingVertical: 10,
          borderBottomWidth: 1,
          borderBottomColor: t.border,
        }}>
        <Label tone="muted">{count.live} live</Label>
        <Label tone={count.working > 0 ? 'signal' : 'faint'}>{count.working} working</Label>
        <Label tone={count.waiting > 0 ? 'warning' : 'faint'} style={{ marginLeft: 'auto' }}>
          {count.waiting} waiting for you
        </Label>
      </View>
      {lions.length === 0 ? <Watermark /> : null}
      <FlashList
        data={lions}
        extraData={beat}
        keyExtractor={(lion) => lion.key}
        contentContainerStyle={{ paddingTop: 10, paddingBottom: 16 }}
        ListEmptyComponent={
          <View style={{ padding: 16, gap: 8 }}>
            <Label tone="faint">
              {online.length === 0 ? '[ No computer online ]' : withDen.length === 0 && asked ? '[ No office ]' : '[ Nobody here ]'}
            </Label>
            <Txt tone="muted">
              {online.length === 0
                ? 'The Office shows the agents that are live on your computers. None of them is reachable now.'
                : withDen.length === 0 && asked
                  ? 'The computers that are online do not share an Office: it needs a newer Leon there, with Share this machine on.'
                  : 'No agent is live on your computers. One that starts there walks in here.'}
            </Txt>
          </View>
        }
        renderItem={({ item: lion }) => {
          const { cub } = lion;
          const color = moodColor(t, lion.mood);
          const can = Boolean(cub.agent && cub.externalId);
          const detail = cub.detail ? fromCwd(cub.detail, cub.cwd) : cub.mystery ? 'Nothing more is known of it there.' : '';
          return (
            <Pressable
              accessibilityRole={can ? 'button' : 'text'}
              accessibilityLabel={`${cub.name}, ${cub.agent || 'agent'} on ${lion.hostName}, ${lion.label}${detail ? `, ${detail}` : ''}`}
              accessibilityHint={can ? 'Opens its chat.' : undefined}
              disabled={!can}
              onPress={() => open(lion)}
              style={({ pressed }) => ({
                flexDirection: 'row',
                alignItems: 'center',
                gap: 12,
                marginHorizontal: 12,
                marginBottom: 6,
                paddingVertical: 10,
                paddingLeft: 10,
                paddingRight: 10,
                backgroundColor: pressed ? t.surface2 : t.surface,
                borderWidth: 1,
                borderColor: lion.mood === 'waiting' ? t.warning : t.border,
                borderLeftWidth: 2,
                borderLeftColor: color,
              })}>
              <View
                style={{
                  width: 76,
                  alignItems: 'center',
                  justifyContent: 'flex-end',
                  backgroundColor: t.background,
                  borderWidth: 1,
                  borderColor: t.border,
                  paddingTop: 4,
                  opacity: lion.mood === 'quiet' ? 0.6 : 1,
                }}>
                <Lion cub={cub} width={68} beat={beat} />
              </View>
              <View style={{ flex: 1, gap: 4 }}>
                <Txt variant="strong" numberOfLines={2}>
                  {cub.name}
                </Txt>
                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                  <Feather name={GLYPH[lion.mood]} size={12} color={color} />
                  <Label style={{ color }}>{lion.label}</Label>
                </View>
                {detail ? (
                  <Txt variant="mono" tone="muted" numberOfLines={2} style={{ fontSize: 12, lineHeight: 17 }}>
                    {detail}
                  </Txt>
                ) : null}
                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                  <Led color={agentColor(t, cub.agent)} />
                  <Label style={{ color: agentColor(t, cub.agent) }}>{cub.agent || 'agent'}</Label>
                  <Label tone="faint" numberOfLines={1} style={{ textTransform: 'none', flex: 1 }}>
                    · {lion.hostName} · Lv {cub.level}
                    {lion.kids.length > 0 ? ` · +${lion.kids.length} sub-agent${lion.kids.length === 1 ? '' : 's'}` : ''}
                  </Label>
                </View>
              </View>
              {can ? <Feather name="chevron-right" size={18} color={t.textMuted} /> : null}
            </Pressable>
          );
        }}
      />
    </Screen>
  );
}
