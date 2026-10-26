// One session in a list: a flat row, as in the desktop's sidebar. A light in
// the agent's colour (filled while something goes on), its title, the agent
// and where it lives in mono under it, and at the end what it is doing or
// how long ago it changed.
import { router } from 'expo-router';
import { Pressable, View } from 'react-native';

import type { ChatRow } from '@/chat/list';
import { useApp } from '@/store/app';
import { agentColor } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';

import { useClock } from './clock';
import { Label, Led, Txt } from './kit';

/** How lately a session must have changed to count as being worked on. */
const MOVING_MS = 90_000;

/** How long ago, in the fewest words. */
export function ago(at: number): string {
  const minutes = Math.max(0, Math.round((Date.now() - at) / 60000));
  if (minutes < 1) return 'now';
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h`;
  return `${Math.round(hours / 24)} d`;
}

/** Opens a row: a shared session gets a record on this device the first time. */
export function openRow(hostId: string, row: ChatRow) {
  let chatId = row.chatId;
  if (!chatId && row.session) {
    const { session } = row;
    chatId = row.key;
    if (!useApp.getState().chats[chatId]) {
      useApp.getState().putChat({
        id: chatId,
        hostId,
        agent: session.agent,
        cwd: session.cwd || undefined,
        title: session.title || 'Untitled',
        externalId: session.externalId,
        resumeId: session.canResume ? session.externalId : undefined,
        sessionIds: [session.externalId],
        createdAt: session.startedMs,
        updatedAt: session.updatedMs,
      });
    }
  }
  if (chatId) router.push({ pathname: '/chat/[chatId]', params: { chatId } });
}

export function SessionCell({
  hostId,
  row,
  where,
}: {
  hostId: string;
  row: ChatRow;
  /** Where it runs, said on a list that mixes computers and projects. */
  where?: string;
}) {
  const t = useTheme();
  const now = useClock();
  const tint = agentColor(t, row.agent);
  // One word at the end, and only when something is going on; otherwise its age.
  const state = row.waiting
    ? { word: 'Waiting', color: t.warning, label: 'Waiting for your answer' }
    : row.running
      ? { word: 'Live', color: t.signal, label: 'Answering' }
      : row.session && now - row.updatedAt < MOVING_MS
        ? // It changed on that computer a moment ago: an agent is at it there.
          { word: 'Active', color: tint, label: 'Changed just now' }
        : null;
  const meta = [row.agent, where ?? (row.session ? `${row.session.messages} msg` : ''), row.canContinue ? '' : 'read only']
    .filter(Boolean)
    .join(' · ');
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${row.title}, ${row.agent}${row.waiting ? ', waiting for your answer' : row.running ? ', answering' : ''}`}
      accessibilityHint={row.canContinue ? 'Opens the chat.' : 'Opens it to read; it cannot be continued from here.'}
      onPress={() => openRow(hostId, row)}
      style={({ pressed }) => ({
        flexDirection: 'row',
        alignItems: 'center',
        gap: 12,
        minHeight: 56,
        paddingVertical: 10,
        paddingLeft: 16,
        paddingRight: 16,
        borderBottomWidth: 1,
        borderBottomColor: t.border,
        backgroundColor: pressed ? t.surface2 : 'transparent',
      })}>
      <Led color={state?.color ?? tint} hollow={!state && !row.running} />
      <View style={{ flex: 1, gap: 3 }}>
        <Txt numberOfLines={1}>{row.title}</Txt>
        <Txt variant="mono" tone="faint" numberOfLines={1} style={{ fontSize: 11, lineHeight: 15 }}>
          {meta}
        </Txt>
      </View>
      {state ? (
        <Label style={{ color: state.color }} accessibilityLabel={state.label}>
          {state.word}
        </Label>
      ) : (
        <Label>{ago(row.updatedAt)}</Label>
      )}
    </Pressable>
  );
}
