// The projects of one computer, as its Leon lists them in the sidebar: the
// first level of its chats. Each opens its worktrees and sessions; the last
// row holds the sessions no project contains.
import Feather from '@expo/vector-icons/Feather';
import { FlashList } from '@shopify/flash-list';
import { router, useFocusEffect, useLocalSearchParams } from 'expo-router';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { ActivityIndicator, Alert, Pressable, RefreshControl, View } from 'react-native';

import {
  allRows,
  buildChatTree,
  pinnedRows,
  searchWords,
  type FoundRow,
  type ProjectNode,
} from '@/chat/list';
import { useApp } from '@/store/app';
import { useKing } from '@/store/king';
import { recheckLogos } from '@/store/logos';
import { ensureLink, forgetHost, refreshShared, useRuntime, watchHost } from '@/store/runtime';
import { useTheme } from '@/theme/useTheme';
import { ConnectionBanner } from '@/ui/ConnectionBanner';
import { Button, IconButton, Label, Led, Rule, Screen, Txt } from '@/ui/kit';
import { Watermark } from '@/ui/Mark';
import { ProjectLogo } from '@/ui/ProjectLogo';
import { SearchField } from '@/ui/SearchField';
import { ago, SessionCell } from '@/ui/SessionCell';

type Item =
  | { type: 'project'; project: ProjectNode }
  | ({ type: 'session' } & FoundRow)
  | { type: 'unsorted'; count: number; folders: number; latest: number };

/** The way to King of this computer: what it does, in a line, and how many workers it has. */
function KingRow({ hostId }: { hostId: string }) {
  const t = useTheme();
  const workers = useKing((s) => s.status[hostId]?.workers.length);
  const busy = useKing((s) => s.status[hostId]?.brain.busy === true);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`King${busy ? ', working' : ''}${workers ? `, ${workers} ${workers === 1 ? 'worker' : 'workers'}` : ''}`}
      accessibilityHint="Opens King: talk to it, and see its workers."
      onPress={() => router.push({ pathname: '/host/[hostId]/king', params: { hostId } })}
      style={({ pressed }) => ({
        flexDirection: 'row',
        alignItems: 'center',
        gap: 12,
        minHeight: 56,
        marginHorizontal: 12,
        marginTop: 12,
        paddingVertical: 10,
        paddingLeft: 14,
        paddingRight: 10,
        backgroundColor: pressed ? t.surface2 : t.surface,
        borderWidth: 1,
        borderColor: pressed ? t.elevatedBorder : t.border,
        borderLeftWidth: 2,
        borderLeftColor: t.signal,
      })}>
      <Feather name="award" size={20} color={t.signal} />
      <View style={{ flex: 1, gap: 3 }}>
        <Txt variant="strong">King</Txt>
        <Txt variant="small" tone="faint" numberOfLines={1}>
          Tell it what to do; it runs the workers
        </Txt>
      </View>
      <View style={{ alignItems: 'flex-end', gap: 4 }}>
        {busy ? (
          <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
            <Led color={t.signal} />
            <Label tone="signal">Working</Label>
          </View>
        ) : null}
        {workers ? (
          <Label tone="faint">
            {workers} {workers === 1 ? 'worker' : 'workers'}
          </Label>
        ) : null}
      </View>
      <Feather name="chevron-right" size={18} color={t.textMuted} />
    </Pressable>
  );
}

export default function HostChats() {
  const { hostId } = useLocalSearchParams<{ hostId: string }>();
  const t = useTheme();
  const host = useApp((s) => s.hosts.find((h) => h.hostId === hostId));
  const chats = useApp((s) => s.chats);
  const shared = useRuntime((s) => s.shared[hostId]);
  const worktrees = useRuntime((s) => s.worktrees[hostId]);
  const online = useRuntime((s) => s.conn[hostId]?.kind === 'online');
  const [refreshing, setRefreshing] = useState(false);

  // While this list is in front, it is read again every little while.
  useFocusEffect(useCallback(() => watchHost(hostId), [hostId]));

  useEffect(() => {
    ensureLink(hostId);
  }, [hostId]);

  const tree = useMemo(
    () =>
      buildChatTree(
        hostId,
        Object.values(chats),
        shared?.sessions ?? [],
        shared?.projects ?? [],
        worktrees,
      ),
    [hostId, chats, shared, worktrees],
  );

  // Searching, or asked for the newest first: every session of the computer
  // in one list, each saying where it lives.
  const [query, setQuery] = useState('');
  const words = useMemo(() => searchWords(query), [query]);
  const order = useApp((s) => s.sessionOrder);
  const setOrder = useApp((s) => s.setSessionOrder);
  const flat = words.length > 0 || order === 'recent';
  const total = tree.projects.reduce((sum, project) => sum + project.count, 0) + tree.unsortedCount;

  // What that computer has pinned, above its projects as in its sidebar.
  const pinned = useMemo(
    () => pinnedRows(hostId, Object.values(chats), shared?.pinned ?? [], tree),
    [hostId, chats, shared, tree],
  );

  const items = useMemo<Item[]>(() => {
    if (flat) return allRows(tree, words).map((found): Item => ({ type: 'session', ...found }));
    const list: Item[] = tree.projects.map((project) => ({ type: 'project', project }));
    if (tree.unsortedCount > 0) {
      list.push({
        type: 'unsorted',
        count: tree.unsortedCount,
        folders: tree.unsorted.length,
        latest: tree.unsorted[0]?.rows[0]?.updatedAt ?? 0,
      });
    }
    return list;
  }, [tree, flat, words]);

  if (!host) {
    return (
      <Screen title="Not found">
        <View style={{ padding: 16 }}>
          <Txt tone="muted">This computer is no longer connected to this device.</Txt>
        </View>
      </Screen>
    );
  }

  const noProjects = shared !== undefined && tree.projects.length === 0;
  // King of this computer, when it has one, above everything but a search.
  const kingRow = shared?.king && words.length === 0 ? <KingRow hostId={hostId} /> : null;

  return (
    <Screen
      kind="Computer"
      title={host.hostName}
      right={
        <IconButton
          icon="trash-2"
          label={`Remove ${host.hostName}`}
          onPress={() =>
            Alert.alert(
              `Remove ${host.hostName}?`,
              'Its chats are removed from this device. Nothing is deleted on that computer. To stop it accepting this device, revoke it there under Share this machine.',
              [
                { text: 'Cancel', style: 'cancel' },
                {
                  text: 'Remove',
                  style: 'destructive',
                  onPress: () => {
                    router.back();
                    forgetHost(hostId);
                  },
                },
              ],
            )
          }
        />
      }>
      <ConnectionBanner hostId={hostId} detail={host.relayUrl.replace(/^wss?:\/\//, '')} />
      {total > 0 ? (
        <SearchField
          query={query}
          onQuery={setQuery}
          placeholder={`Search ${total} ${total === 1 ? 'session' : 'sessions'}`}
          order={order}
          onOrder={setOrder}
          grouped="By project"
        />
      ) : null}
      {items.length === 0 && !flat ? <Watermark /> : null}
      <FlashList
        data={items}
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        keyExtractor={(item) =>
          item.type === 'project' ? item.project.key : item.type === 'session' ? item.row.key : '[unsorted]'
        }
        getItemType={(item) => item.type}
        ListHeaderComponent={
          <>
            {kingRow}
            {words.length > 0 ? (
              <View style={{ paddingHorizontal: 16, paddingTop: 16, paddingBottom: 8 }}>
                <Label tone="faint">[ {items.length} found ]</Label>
              </View>
            ) : flat || tree.projects.length > 0 || pinned.length > 0 ? (
              <View>
                {pinned.length > 0 ? (
                  <View style={{ paddingTop: 16, paddingBottom: 4 }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8, paddingHorizontal: 16, paddingBottom: 8 }}>
                      <Feather name="bookmark" size={12} color={t.signal} />
                      <Label tone="faint">[ Pinned ]</Label>
                      <Label tone="faint" style={{ marginLeft: 'auto' }}>
                        {pinned.length}
                      </Label>
                    </View>
                    {pinned.map(({ row, where }) => (
                      <SessionCell key={row.key} hostId={hostId} row={row} where={where} />
                    ))}
                  </View>
                ) : null}
                {flat || tree.projects.length > 0 ? (
                  <View style={{ paddingHorizontal: 16, paddingTop: 16, paddingBottom: 8 }}>
                    <Label tone="faint">{flat ? '[ Recent ]' : '[ Projects ]'}</Label>
                  </View>
                ) : null}
              </View>
            ) : noProjects ? (
              // The whole of what a computer without projects has to say.
              <View style={{ paddingHorizontal: 16, paddingTop: 16, paddingBottom: 12, gap: 6 }}>
                <Label tone="faint">[ No projects ]</Label>
                <Txt variant="small" tone="muted">
                  This computer shares no projects: add them in its Leon and they are listed
                  here. Chats you start from this device work either way.
                </Txt>
              </View>
            ) : null}
          </>
        }
        refreshControl={
          <RefreshControl
            refreshing={refreshing}
            tintColor={t.textMuted}
            onRefresh={() => {
              setRefreshing(true);
              recheckLogos(hostId);
              void refreshShared(hostId).finally(() => setRefreshing(false));
            }}
          />
        }
        contentContainerStyle={{ paddingBottom: 16 }}
        ListEmptyComponent={
          flat ? (
            <View style={{ paddingHorizontal: 16 }}>
              <Txt tone="muted">
                {words.length > 0 ? 'No session of this computer is called that.' : 'No sessions yet.'}
              </Txt>
            </View>
          ) : !shared && online ? (
            <View style={{ padding: 24 }}>
              <ActivityIndicator color={t.textMuted} />
            </View>
          ) : noProjects ? (
            <View style={{ paddingHorizontal: 16 }}>
              <Txt tone="faint">No chats yet. Start one below: pick an agent and a folder, and ask.</Txt>
            </View>
          ) : null
        }
        ListFooterComponent={
          shared?.truncated ? (
            <View style={{ padding: 16 }}>
              <Txt variant="small" tone="faint">
                Only the most recent sessions of this computer are listed.
              </Txt>
            </View>
          ) : null
        }
        renderItem={({ item }) => {
          if (item.type === 'session') {
            return <SessionCell hostId={hostId} row={item.row} where={item.where} />;
          }
          if (item.type === 'unsorted') {
            // A bucket, not a project: a bracketed label on a dashed outline,
            // set apart from the projects above it.
            return (
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`Unsorted, ${item.count} ${item.count === 1 ? 'session' : 'sessions'} in ${item.folders} ${item.folders === 1 ? 'folder' : 'folders'}`}
                accessibilityHint="Opens the sessions that belong to no project."
                onPress={() => router.push({ pathname: '/host/[hostId]/unsorted', params: { hostId } })}
                style={({ pressed }) => ({
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 12,
                  minHeight: 52,
                  marginHorizontal: 12,
                  marginTop: tree.projects.length > 0 ? 14 : 0,
                  paddingVertical: 10,
                  paddingLeft: 14,
                  paddingRight: 10,
                  backgroundColor: pressed ? t.surface2 : 'transparent',
                  borderWidth: 1,
                  borderStyle: 'dashed',
                  borderColor: pressed ? t.elevatedBorder : t.border,
                })}>
                <View style={{ flex: 1, gap: 3 }}>
                  <Label tone="muted">[ Unsorted ]</Label>
                  <Txt variant="small" tone="faint" numberOfLines={1}>
                    Sessions outside every project
                  </Txt>
                </View>
                <View style={{ alignItems: 'flex-end', gap: 4 }}>
                  <Label>{ago(item.latest)}</Label>
                  <Label tone="faint">
                    {item.count} {item.count === 1 ? 'session' : 'sessions'}
                  </Label>
                </View>
                <Feather name="chevron-right" size={18} color={t.textMuted} />
              </Pressable>
            );
          }
          const { project } = item;
          const { count, trees } = project;
          return (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={`${project.title}, ${count} ${count === 1 ? 'session' : 'sessions'}${trees > 1 ? `, ${trees} worktrees` : ''}${project.waiting ? ', waiting for your answer' : project.running ? ', answering' : ''}`}
              accessibilityHint="Opens its worktrees and sessions."
              onPress={() =>
                router.push({ pathname: '/host/[hostId]/project', params: { hostId, project: project.key } })
              }
              style={({ pressed }) => ({
                flexDirection: 'row',
                alignItems: 'center',
                gap: 12,
                minHeight: 64,
                marginHorizontal: 12,
                marginBottom: 6,
                paddingVertical: 12,
                paddingLeft: 14,
                paddingRight: 10,
                backgroundColor: pressed ? t.surface2 : t.surface,
                borderWidth: 1,
                borderColor: pressed ? t.elevatedBorder : t.border,
              })}>
              <ProjectLogo hostId={hostId} root={project.path} size={28} />
              <View style={{ flex: 1, gap: 3 }}>
                <Txt variant="strong" numberOfLines={1}>
                  {project.title}
                </Txt>
                <Txt variant="mono" tone="faint" numberOfLines={1} ellipsizeMode="head" style={{ fontSize: 11, lineHeight: 15 }}>
                  {project.path}
                </Txt>
              </View>
              <View style={{ alignItems: 'flex-end', gap: 4 }}>
                {project.waiting ? (
                  <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                    <Led color={t.warning} />
                    <Label tone="warning">Waiting</Label>
                  </View>
                ) : project.running ? (
                  <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                    <Led color={t.signal} />
                    <Label tone="signal">Live</Label>
                  </View>
                ) : count > 0 ? (
                  <Label>{ago(project.latest)}</Label>
                ) : null}
                <Label tone="faint">
                  {count} {count === 1 ? 'session' : 'sessions'}
                  {trees > 1 ? ` · ${trees} trees` : ''}
                </Label>
              </View>
              <Feather name="chevron-right" size={18} color={t.textMuted} />
            </Pressable>
          );
        }}
      />
      <Rule />
      <View style={{ padding: 16 }}>
        <Button
          kind="primary"
          title="New chat"
          icon="plus"
          onPress={() => router.push({ pathname: '/host/[hostId]/new', params: { hostId } })}
        />
      </View>
    </Screen>
  );
}
