// One chat: what was said, the answer as it is written, and the composer.
import Feather from '@expo/vector-icons/Feather';
import { FlashList, type FlashListRef } from '@shopify/flash-list';
import { useFocusEffect, useLocalSearchParams } from 'expo-router';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Alert,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  Pressable,
  ScrollView,
  StyleSheet,
  TextInput,
  View,
} from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import {
  fitOptions,
  isDangerous,
  modeLabel,
  optionsLine,
  withoutLegacyMode,
  type Message,
} from '@/chat/model';
import { withAttached } from '@/chat/attach';
import { isWaiting, pendingQuestions } from '@/chat/reduce';
import { activityWord, withoutNotes } from '@/chat/transcript';
import type { AgentOptions, TurnOptions } from '@/link/types';
import { chatInFront } from '@/store/alerts';
import { optionsKey, useApp } from '@/store/app';
import { takeAttached, useAttachments } from '@/store/attachments';
import {
  answer,
  combineRows,
  ensureLink,
  kill,
  link,
  loadAgentOptions,
  loadMessages,
  loadTranscript,
  send,
  setOptions,
  stop,
  useRuntime,
  watchChat,
} from '@/store/runtime';
import { agentColor, fonts, metrics } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';
import { AttachedRow, AttachSheet, useClipboardImage } from '@/ui/Attach';
import { ConnectionBanner } from '@/ui/ConnectionBanner';
import { IconButton, Label, Led, Rule, Screen, Txt } from '@/ui/kit';
import { MessageRow } from '@/ui/MessageRow';
import { OptionsSheet } from '@/ui/OptionsSheet';
import { QuestionPanel } from '@/ui/QuestionPanel';

/** What was being written in each chat, kept while the application lives. */
const drafts = new Map<string, string>();

/** Things asked often, a press away when the composer is empty. They fill it; nothing is sent. */
const SHORTCUTS = [
  ['Continue', 'Continue.'],
  ['What changed?', 'Summarise what you changed so far, file by file, and what is left to do.'],
  ['Run the tests', 'Run the tests and fix what fails.'],
  ['Commit', 'Commit what you changed, with a message that says why.'],
  ['Open a PR', 'Push this branch and open a pull request.'],
] as const;

/** When the turn of this id was asked for: its id carries it. */
function startedAt(turnId: string | undefined): number | undefined {
  const stamp = turnId?.split('#').pop();
  const at = stamp ? parseInt(stamp, 36) : NaN;
  return Number.isFinite(at) && at > 0 ? at : undefined;
}

function clock(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, '0')}`;
}

/** The line under a turn in flight: who is at it and for how long. It counts by itself. */
function Working({
  word,
  color,
  since,
  hollow,
}: {
  word: string;
  color: string;
  since?: number;
  hollow: boolean;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === undefined) return;
    const timer = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(timer);
  }, [since]);
  return (
    <View style={styles.working}>
      <Led color={color} hollow={hollow} />
      <Label style={{ color, flex: 1 }} accessibilityLiveRegion="polite">
        {word}
      </Label>
      {since !== undefined ? <Label tone="faint">{clock(now - since)}</Label> : null}
    </View>
  );
}

/** Asks before a turn in a mode that lets the agent do anything; runs `go` at once otherwise. */
function confirmed(offered: AgentOptions | undefined, options: TurnOptions, go: () => void) {
  if (!isDangerous(offered, options.mode)) {
    go();
    return;
  }
  Alert.alert(
    'Let the agent do anything?',
    `In ${modeLabel(offered, options.mode).toLowerCase()} it may run any command and change any file on that computer, as you, without asking. There is no sandbox.`,
    [
      { text: 'Cancel', style: 'cancel' },
      { text: 'Send', style: 'destructive', onPress: go },
    ],
  );
}

export default function Chat() {
  const { chatId } = useLocalSearchParams<{ chatId: string }>();
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const chat = useApp((s) => s.chats[chatId]);
  const transcript = useRuntime((s) => s.transcripts[chatId]);
  const local = useRuntime((s) => s.messages[chatId]);
  const turn = useRuntime((s) => s.turns[chatId]);
  const lastTokens = useRuntime((s) => s.lastTokens[chatId]);
  const hostId = chat?.hostId ?? '';
  const online = useRuntime((s) => s.conn[hostId]?.kind === 'online');

  const activity = useRuntime((s) => s.activity[chatId]);

  // The agent as this app can drive it; nothing when it cannot.
  const driver = useMemo(() => link().agents().find((a) => a.id === chat?.agent), [chat?.agent]);
  const agentName = driver?.name ?? chat?.agent ?? '';

  // What the agent offers on that computer, asked once the computer answers.
  const offeredKey = optionsKey(hostId, chat?.agent ?? '');
  const offered = useRuntime((s) => s.options[offeredKey]);
  const lastUsed = useApp((s) => s.lastOptions[offeredKey]);
  const agentTag = driver ? chat?.agent : undefined;
  useEffect(() => {
    if (hostId && agentTag && online) void loadAgentOptions(hostId, agentTag);
  }, [hostId, agentTag, online]);

  const [draft, setDraftText] = useState(() => drafts.get(chatId) ?? '');
  const setDraft = useCallback(
    (text: string) => {
      setDraftText(text);
      if (text) drafts.set(chatId, text);
      else drafts.delete(chatId);
    },
    [chatId],
  );
  // A chat that never ran starts with what was last used with this agent
  // there. What the agent no longer offers is never asked for.
  const options = useMemo(
    () => fitOptions(offered, withoutLegacyMode(chat?.options ?? lastUsed)),
    [offered, chat?.options, lastUsed],
  );
  const [choosing, setChoosing] = useState(false);
  const [attaching, setAttaching] = useState(false);
  // What goes with the next message: each file is on the computer once kept.
  const attached = useAttachments((s) => s.byChat[chatId]);
  const keptCount = attached?.filter((a) => a.state === 'kept').length ?? 0;
  const stillSending = attached?.some((a) => a.state === 'sending') ?? false;
  // The turn that was asked to stop: the next press of the same button kills it.
  const [stopAsked, setStopAsked] = useState<string | null>(null);
  // System notes: hidden until asked for, and each one closed until opened.
  const [showNotes, setShowNotes] = useState(false);
  const [openNotes, setOpenNotes] = useState<ReadonlySet<string>>(() => new Set());
  const toggleNote = useCallback((id: string) => {
    setOpenNotes((before) => {
      const next = new Set(before);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  useEffect(() => {
    loadMessages(chatId);
    if (hostId) ensureLink(hostId);
  }, [chatId, hostId]);

  // The shared transcript, read when the chat opens and again when the
  // computer comes back.
  const externalId = chat?.externalId;
  useEffect(() => {
    const current = useApp.getState().chats[chatId];
    if (current && externalId && online) void loadTranscript(current);
  }, [chatId, externalId, online]);

  // While this screen is in front, the session is followed on its computer.
  useFocusEffect(useCallback(() => watchChat(chatId), [chatId]));
  // What happens here is seen here: it is not announced on top of it.
  useFocusEffect(useCallback(() => chatInFront(chatId), [chatId]));

  // The word about the session's progress ages: say it again every few seconds.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!externalId) return;
    const timer = setInterval(() => setNow(Date.now()), 5_000);
    return () => clearInterval(timer);
  }, [externalId]);
  const progress = activityWord(activity, now);

  const all = useMemo(() => combineRows(transcript, local, turn), [transcript, local, turn]);
  const noteCount = useMemo(() => all.reduce((n, m) => n + (m.kind === 'note' ? 1 : 0), 0), [all]);
  const rows = useMemo(() => (showNotes ? all : withoutNotes(all)), [all, showNotes]);

  const running = turn !== undefined || chat?.activeTurn !== undefined;
  const readOnly = chat !== undefined && !driver;
  const [canPaste, pasted] = useClipboardImage(!readOnly && online);
  const questions = useMemo(() => pendingQuestions(turn), [turn]);
  const waiting = isWaiting(turn);
  // Whether it reads more while it works is the agent's to say.
  const canSayMore = turn !== undefined && (offered?.followUps ?? false);
  const stopping = stopAsked !== null && stopAsked === (turn?.turnId ?? chat?.activeTurn?.turnId);

  // Rows that arrive while the reader is further up do not move the page:
  // a small button says there is more below.
  const list = useRef<FlashListRef<Message>>(null);
  const atEnd = useRef(true);
  const seen = useRef(rows.length);
  const count = useRef(rows.length);
  const [unseen, setUnseen] = useState(0);
  useEffect(() => {
    count.current = rows.length;
    if (atEnd.current) seen.current = rows.length;
    setUnseen(Math.max(0, rows.length - seen.current));
  }, [rows.length]);
  const onScroll = useCallback((event: NativeSyntheticEvent<NativeScrollEvent>) => {
    const { contentOffset, contentSize, layoutMeasurement } = event.nativeEvent;
    atEnd.current = contentSize.height - layoutMeasurement.height - contentOffset.y < 120;
    if (atEnd.current) {
      seen.current = count.current;
      setUnseen(0);
    }
  }, []);

  const renderItem = useCallback(
    ({ item }: { item: Message }) => (
      <MessageRow
        message={item}
        agent={chat?.agent ?? ''}
        agentName={agentName}
        cwd={chat?.cwd}
        open={openNotes.has(item.id)}
        onToggle={toggleNote}
      />
    ),
    [chat?.agent, chat?.cwd, agentName, openNotes, toggleNote],
  );

  if (!chat) {
    return (
      <Screen title="Not found">
        <View style={{ padding: 16 }}>
          <Txt tone="muted">This chat is no longer on this device.</Txt>
        </View>
      </Screen>
    );
  }

  // While the agent works, a message is added to what it has to answer, if
  // it can be told more. It is kept and sent even when written offline.
  // Files go with it once every one is on the computer.
  const canSend =
    (draft.trim().length > 0 || keptCount > 0) && !stillSending && (running ? canSayMore : online);

  function submit() {
    if (!canSend) return;
    const go = () => {
      const text = withAttached(draft, takeAttached(chatId));
      setDraft('');
      void send(chatId, text, options);
    };
    if (running) {
      go();
      return;
    }
    confirmed(offered, options, go);
  }

  function askToStop() {
    if (!stopping) {
      setStopAsked(turn?.turnId ?? chat?.activeTurn?.turnId ?? null);
      stop(chatId);
      return;
    }
    forceStop();
  }

  function forceStop() {
    Alert.alert(
      `Kill ${agentName}?`,
      'It is stopped at once, in the middle of whatever it is doing. Asking it to stop lets it finish the step it is on.',
      [
        { text: 'Cancel', style: 'cancel' },
        { text: 'Kill', style: 'destructive', onPress: () => kill(chatId) },
      ],
    );
  }

  // The mode the agent says it is in now, while it runs; the chosen one otherwise.
  const liveMode = turn?.mode && offered?.modes.some((m) => m.id === turn.mode) ? turn.mode : options.mode;
  const shown = turn ? { ...options, mode: liveMode } : options;
  const dangerous = isDangerous(offered, shown.mode);

  return (
    <Screen
      kind={chat.agent}
      title={chat.title || 'New chat'}
      subtitle={chat.cwd ?? 'default folder'}
      bottomInset={false}
      right={
        // What programs wrote into the conversation (task notifications,
        // reminders, commands): hidden until asked for.
        noteCount > 0 ? (
          <IconButton
            icon={showNotes ? 'eye' : 'eye-off'}
            label={`${showNotes ? 'Hide' : 'Show'} system notes, ${noteCount} in this chat`}
            color={showNotes ? t.signal : t.textFaint}
            onPress={() => setShowNotes((on) => !on)}
          />
        ) : undefined
      }>
      <ConnectionBanner
        hostId={hostId}
        detail={waiting ? 'Waiting for you' : progress.word || modeLabel(offered, shown.mode).toLowerCase()}
        detailLive={progress.live}
        detailWaits={waiting}
      />
      <KeyboardAvoidingView
        behavior="padding"
        keyboardVerticalOffset={-insets.bottom}
        style={{ flex: 1 }}>
        <View style={{ flex: 1 }}>
        <FlashList
          ref={list}
          data={rows}
          renderItem={renderItem}
          extraData={openNotes}
          keyExtractor={(item) => item.id}
          getItemType={(item) => item.kind}
          onScroll={onScroll}
          scrollEventThrottle={100}
          keyboardDismissMode="interactive"
          keyboardShouldPersistTaps="handled"
          maintainVisibleContentPosition={{
            autoscrollToBottomThreshold: 0.25,
            startRenderingFromBottom: true,
          }}
          contentContainerStyle={{ paddingTop: 4, paddingBottom: 16 }}
          ListEmptyComponent={
            <View style={{ padding: 24, gap: 8 }}>
              <Label tone="muted">[ Nothing said yet ]</Label>
              <Txt variant="strong">Ask {agentName} something.</Txt>
              <Txt tone="muted">
                It works in {chat.cwd ?? 'the folder Leon runs in'} on that computer, and keeps
                going if this phone disconnects.
              </Txt>
            </View>
          }
          ListFooterComponent={
            // A question is asked right below: it needs no line of its own.
            running && !(waiting && online) ? (
              <Working
                since={startedAt(turn?.turnId ?? chat.activeTurn?.turnId)}
                hollow={!online}
                color={online ? agentColor(t, chat.agent) : t.textFaint}
                word={online ? `${agentName} is working` : 'Waiting for the connection'}
              />
            ) : !running && lastTokens ? (
              // The foot of the last answer: what it read and wrote, and how fast.
              <Label tone="faint" style={styles.foot} accessibilityLabel={`Last answer: ${lastTokens}`}>
                {lastTokens}
              </Label>
            ) : null
          }
        />
        {unseen > 0 ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${unseen} new below. Go to the end.`}
            onPress={() => {
              atEnd.current = true;
              seen.current = count.current;
              setUnseen(0);
              void list.current?.scrollToEnd({ animated: true });
            }}
            style={({ pressed }) => [
              styles.more,
              { backgroundColor: pressed ? t.surface2 : t.surface, borderColor: t.signal },
            ]}>
            <Feather name="arrow-down" size={14} color={t.signal} />
            <Label tone="signal">{unseen} new</Label>
          </Pressable>
        ) : null}
        </View>
        <Rule />
        {readOnly ? (
          <View style={{ padding: 16, paddingBottom: insets.bottom + 16 }}>
            <Txt variant="small" tone="faint">
              This session can be read here but not continued: this version drives Claude Code,
              Codex and opencode, not {chat.agent}.
            </Txt>
          </View>
        ) : (
          <View style={{ backgroundColor: t.surface, paddingBottom: insets.bottom }}>
            <QuestionPanel
              questions={questions}
              agentName={agentName}
              online={online}
              onAnswer={(questionId, reply) => answer(chatId, questionId, reply)}
            />
            {!running && online && draft.length === 0 && rows.length > 0 ? (
              <ScrollView
                horizontal
                showsHorizontalScrollIndicator={false}
                keyboardShouldPersistTaps="handled"
                contentContainerStyle={styles.shortcuts}>
                {SHORTCUTS.map(([title, text]) => (
                  <Pressable
                    key={title}
                    accessibilityRole="button"
                    accessibilityLabel={title}
                    accessibilityHint="Writes it in the message. Nothing is sent yet."
                    onPress={() => setDraft(text)}
                    style={({ pressed }) => [
                      styles.shortcut,
                      { borderColor: t.border, backgroundColor: pressed ? t.surface2 : t.background },
                    ]}>
                    <Label tone="muted">{title}</Label>
                  </Pressable>
                ))}
              </ScrollView>
            ) : null}
            <AttachedRow chatId={chatId} canPaste={canPaste} onPasted={pasted} />
            <View style={styles.inputRow}>
              <Txt variant="mono" tone={online ? 'signal' : 'faint'} style={styles.prompt}>
                {'>'}
              </Txt>
              <TextInput
                accessibilityLabel="Message"
                value={draft}
                onChangeText={setDraft}
                placeholder={
                  running && canSayMore
                    ? `Tell ${agentName} more`
                    : online
                      ? `Ask ${agentName}`
                      : 'Not connected'
                }
                placeholderTextColor={t.textFaint}
                selectionColor={t.signal}
                cursorColor={t.signal}
                multiline
                style={[styles.input, { color: t.text }]}
              />
              {!running || (canSayMore && (draft.trim().length > 0 || keptCount > 0)) ? (
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={running ? 'Tell it more' : 'Send'}
                  accessibilityState={{ disabled: !canSend }}
                  disabled={!canSend}
                  onPress={submit}
                  style={({ pressed }) => [
                    styles.action,
                    // One accent on the screen: while something is asked,
                    // it is the answer.
                    canSend && questions.length === 0
                      ? { backgroundColor: t.accentFill, opacity: pressed ? 0.85 : 1 }
                      : {
                          backgroundColor: pressed && canSend ? t.surface : t.surface2,
                          borderWidth: 1,
                          borderColor: canSend ? t.elevatedBorder : t.border,
                        },
                  ]}>
                  <Feather
                    name="arrow-up"
                    size={20}
                    color={!canSend ? t.textFaint : questions.length === 0 ? t.onAccentFill : t.text}
                  />
                </Pressable>
              ) : null}
              {running ? (
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={stopping ? 'Kill' : 'Stop'}
                  accessibilityHint={
                    stopping
                      ? 'It was asked to stop. Press again to kill it at once.'
                      : 'Asks the agent to stop. Hold to kill it at once.'
                  }
                  disabled={!online}
                  onPress={askToStop}
                  onLongPress={forceStop}
                  style={({ pressed }) => [
                    styles.action,
                    {
                      borderWidth: 1,
                      borderColor: !online ? t.border : stopping ? t.error : t.elevatedBorder,
                      backgroundColor: pressed ? t.surface2 : 'transparent',
                    },
                  ]}>
                  <Feather
                    name={stopping ? 'x' : 'square'}
                    size={16}
                    color={!online ? t.textFaint : stopping ? t.error : t.text}
                  />
                </Pressable>
              ) : null}
            </View>
            {/* Under the field, small: what to attach, and how the agent runs. */}
            <View style={[styles.tools, { borderTopColor: t.border }]}>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel="Attach a photo or a file"
                accessibilityState={{ disabled: !online }}
                disabled={!online}
                hitSlop={4}
                onPress={() => setAttaching(true)}
                style={({ pressed }) => [styles.tool, { backgroundColor: pressed ? t.surface2 : 'transparent' }]}>
                <Feather name="paperclip" size={16} color={online ? t.textMuted : t.textFaint} />
              </Pressable>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`Mode, model and effort: ${optionsLine(offered, shown)}`}
                accessibilityHint="Changes how the agent runs: what it may do without asking, its model, its effort."
                onPress={() => setChoosing(true)}
                style={({ pressed }) => [
                  styles.tool,
                  styles.modeTool,
                  { backgroundColor: pressed ? t.surface2 : 'transparent' },
                ]}>
                <Led color={dangerous ? t.warning : t.textMuted} hollow={!shown.mode} />
                <Label tone={dangerous ? 'warning' : 'muted'} numberOfLines={1} style={{ flexShrink: 1 }}>
                  {dangerous ? '! ' : ''}
                  {optionsLine(offered, shown)}
                </Label>
                <Feather name="chevron-up" size={14} color={t.textFaint} />
              </Pressable>
            </View>
          </View>
        )}
      </KeyboardAvoidingView>
      <AttachSheet
        chatId={chatId}
        visible={attaching}
        canPaste={canPaste}
        onPasted={pasted}
        onClose={() => setAttaching(false)}
      />
      <OptionsSheet
        visible={choosing}
        agentName={agentName}
        offered={offered}
        value={options}
        running={turn !== undefined}
        onChange={(next) => setOptions(chatId, next)}
        onClose={() => setChoosing(false)}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  foot: { marginHorizontal: metrics.pad, marginTop: 10, textTransform: 'none', letterSpacing: 0 },
  more: {
    position: 'absolute',
    alignSelf: 'center',
    bottom: 12,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    minHeight: 32,
    paddingHorizontal: 12,
    borderWidth: 1,
    borderRadius: metrics.radius,
  },
  working: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginHorizontal: metrics.pad,
    marginTop: 14,
    minHeight: 24,
  },
  shortcuts: { paddingHorizontal: metrics.pad, paddingTop: 8, gap: 8 },
  shortcut: {
    minHeight: 36,
    paddingHorizontal: 12,
    justifyContent: 'center',
    borderWidth: 1,
    borderRadius: metrics.radius,
  },
  tools: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderTopWidth: 1,
  },
  tool: {
    minHeight: 40,
    minWidth: 40,
    paddingHorizontal: 8,
    borderRadius: metrics.radius,
    alignItems: 'center',
    justifyContent: 'center',
  },
  modeTool: { flexShrink: 1, flexDirection: 'row', gap: 8 },
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
