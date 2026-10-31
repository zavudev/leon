// The computers this device paired with, and whether each is reachable.
import Feather from '@expo/vector-icons/Feather';
import { router, useFocusEffect } from 'expo-router';
import { useCallback, useMemo } from 'react';
import { Alert, Pressable, ScrollView, View } from 'react-native';

import { lionsOf, tally } from '@/chat/den';
import type { ChatRecord, ChatRow } from '@/chat/list';

import { connectionWord } from '@/link/errors';
import { useApp, type HostRecord } from '@/store/app';
import { addDemo, askDens, forgetHost, useRuntime } from '@/store/runtime';
import { useTheme } from '@/theme/useTheme';
import { connectionColor } from '@/ui/ConnectionBanner';
import { Button, Label, Led, Row, Rule, Screen, Section, Signature, Txt } from '@/ui/kit';
import { Lion } from '@/ui/Lion';
import { Mark, Watermark } from '@/ui/Mark';
import { SessionCell } from '@/ui/SessionCell';

function confirmRemove(host: HostRecord) {
  Alert.alert(
    `Remove ${host.hostName}?`,
    'Its chats are removed from this device. Nothing is deleted on that computer. To stop it accepting this device, revoke it there under Share this machine.',
    [
      { text: 'Cancel', style: 'cancel' },
      { text: 'Remove', style: 'destructive', onPress: () => forgetHost(host.hostId) },
    ],
  );
}

/** How long after the first question the home asks the dens again: the first answer may hold nobody. */
const DEN_AGAIN_MS = 4_000;

/**
 * The way into the Den, said with who is in it: a few of its lions, those
 * that wait first, and the count. It is the one place of the home drawn in
 * the accent.
 */
function DenCard() {
  const t = useTheme();
  const hosts = useApp((s) => s.hosts);
  const dens = useRuntime((s) => s.dens);
  const lions = useMemo(() => lionsOf(hosts, dens), [hosts, dens]);
  const count = tally(lions);
  const shown = lions.slice(0, 4);

  useFocusEffect(
    useCallback(() => {
      void askDens();
      const again = setTimeout(() => void askDens(), DEN_AGAIN_MS);
      return () => clearTimeout(again);
    }, []),
  );

  const waiting = count.waiting > 0;
  const edge = waiting ? t.warning : t.signal;
  const says =
    count.live === 0
      ? 'Every agent that is live on your computers, and what it is doing.'
      : null;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={
        count.live === 0
          ? 'The Office'
          : `The Office, ${count.live} live, ${count.working} working, ${count.waiting} waiting for you`
      }
      accessibilityHint="Shows every agent that is live on your computers."
      onPress={() => router.push('/den')}
      style={({ pressed }) => ({
        marginHorizontal: 12,
        marginTop: 16,
        backgroundColor: pressed ? t.surface2 : t.surface,
        borderWidth: 1,
        borderColor: edge,
        borderLeftWidth: 3,
      })}>
      <View style={{ flexDirection: 'row', alignItems: 'center', gap: 12, paddingVertical: 12, paddingLeft: 14, paddingRight: 10 }}>
        <View style={{ flex: 1, gap: 6 }}>
          <Label style={{ color: edge }}>[ The Office ]</Label>
          {says ? (
            <Txt variant="small" tone="muted">
              {says}
            </Txt>
          ) : (
            <View style={{ gap: 3 }}>
              <Txt variant="strong">
                {waiting
                  ? `${count.waiting} waiting for you`
                  : count.working > 0
                    ? `${count.working} working`
                    : `${count.live} resting`}
              </Txt>
              <Label tone="faint">
                {count.live} live · {count.working} working
              </Label>
            </View>
          )}
        </View>
        {shown.length > 0 ? (
          <View style={{ flexDirection: 'row', alignItems: 'flex-end' }}>
            {shown.map((lion, at) => (
              <View key={lion.key} style={{ marginLeft: at === 0 ? 0 : -14, opacity: lion.mood === 'quiet' ? 0.55 : 1 }}>
                <Lion cub={lion.cub} width={40} beat={0} />
              </View>
            ))}
          </View>
        ) : null}
        <Feather name="chevron-right" size={18} color={edge} />
      </View>
    </Pressable>
  );
}

function HostRow({ host }: { host: HostRecord }) {
  const t = useTheme();
  const state = useRuntime((s) => s.conn[host.hostId]);
  const word = connectionWord(state);
  const color = connectionColor(t, state?.kind);
  return (
    <Row
      accessibilityLabel={`${host.hostName}, ${word}`}
      accessibilityHint="Opens its chats. Hold to remove it."
      onPress={() => router.push({ pathname: '/host/[hostId]', params: { hostId: host.hostId } })}
      onLongPress={() => confirmRemove(host)}>
      <Led color={color} hollow={state?.kind !== 'online'} />
      <View style={{ flex: 1, gap: 2 }}>
        <Txt variant="strong" numberOfLines={1}>
          {host.hostName}
        </Txt>
        <Txt variant="mono" tone="faint" numberOfLines={1} ellipsizeMode="middle">
          {host.relayUrl.replace(/^wss?:\/\//, '')}
        </Txt>
      </View>
      <Label style={{ color }}>{word}</Label>
      <Feather name="chevron-right" size={16} color={t.textFaint} />
    </Row>
  );
}

/** How many chats the home lists under "recent". */
const RECENT = 4;

function asRow(chat: ChatRecord): ChatRow {
  return {
    key: chat.id,
    chatId: chat.id,
    agent: chat.agent,
    cwd: chat.cwd ?? '',
    title: chat.title || 'New chat',
    updatedAt: chat.updatedAt,
    canContinue: true,
    running: chat.activeTurn !== undefined,
    waiting: chat.activeTurn?.waiting === true,
  };
}

/** The last name of a folder: what a project is called at a glance. */
function folderName(path: string | undefined): string {
  return path?.replace(/[\/]+$/, '').split(/[\/]/).pop() ?? '';
}

export default function Hosts() {
  const hosts = useApp((s) => s.hosts);
  const chats = useApp((s) => s.chats);
  // What needs the person comes first, wherever it runs: an agent that asked
  // something, then the ones at work, then where they last were.
  const { waiting, working, recent } = useMemo(() => {
    const all = Object.values(chats).sort((a, b) => b.updatedAt - a.updatedAt);
    return {
      waiting: all.filter((c) => c.activeTurn?.waiting),
      working: all.filter((c) => c.activeTurn && !c.activeTurn.waiting),
      recent: all.filter((c) => !c.activeTurn).slice(0, RECENT),
    };
  }, [chats]);
  const cell = (chat: ChatRecord) => {
    const host = hosts.find((h) => h.hostId === chat.hostId);
    const place = [host?.hostName, folderName(chat.cwd)].filter(Boolean).join(' / ');
    return <SessionCell key={chat.id} hostId={chat.hostId} row={asRow(chat)} where={place} />;
  };

  if (hosts.length === 0) {
    return (
      <Screen title="Leon" brand back={false} bottomInset={false}>
        <Watermark size={340} />
        <View style={{ flex: 1, padding: 24, justifyContent: 'center', gap: 14 }}>
          <Mark size={64} />
          <Label tone="muted">[ No computer yet ]</Label>
          <Txt variant="title">Ask your agents from here.</Txt>
          <Txt tone="muted">
            Connect a computer that runs Leon. You send a prompt from this phone, the agent works
            over there, and the answer comes back here.
          </Txt>
          <Txt variant="small" tone="faint">
            The connection is end-to-end encrypted with keys only the two devices hold, so the
            relay only passes along bytes it cannot read.
          </Txt>
          <Button
            kind="primary"
            title="Connect a computer"
            icon="link"
            onPress={() => router.push('/pair')}
            style={{ marginTop: 10 }}
          />
          <Button
            title="Try a demo"
            icon="play"
            accessibilityHint="Adds a made-up computer, to see how the app works before connecting yours"
            onPress={() =>
              router.push({ pathname: '/host/[hostId]', params: { hostId: addDemo() } })
            }
          />
        </View>
        <Signature style={{ paddingBottom: 14 }} />
      </Screen>
    );
  }

  return (
    <Screen title="Leon" brand back={false} bottomInset={false}>
      <Watermark size={340} />
      <ScrollView contentContainerStyle={{ flexGrow: 1, paddingBottom: 12 }}>
        {waiting.length > 0 ? (
          <>
            <Section title="Waiting for you" right={<Label tone="warning">{waiting.length}</Label>} />
            {waiting.map(cell)}
          </>
        ) : null}
        {working.length > 0 ? (
          <>
            <Section title="Working" right={<Label tone="signal">{working.length}</Label>} />
            {working.map(cell)}
          </>
        ) : null}
        {recent.length > 0 ? (
          <>
            <Section title="Recent" />
            {recent.map(cell)}
          </>
        ) : null}
        <DenCard />
        <Section title="Computers" right={<Label>{hosts.length}</Label>} />
        <Rule />
        {hosts.map((host) => (
          <HostRow key={host.hostId} host={host} />
        ))}
        <View style={{ padding: 16 }}>
          <Button title="Connect another computer" icon="plus" onPress={() => router.push('/pair')} />
        </View>
        <View style={{ flex: 1 }} />
        <Signature />
      </ScrollView>
    </Screen>
  );
}
