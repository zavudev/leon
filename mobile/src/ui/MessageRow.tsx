// One row of a chat: what you asked, what the agent wrote, a tool it used, or
// a note about the turn.
import Feather from '@expo/vector-icons/Feather';
import * as Clipboard from 'expo-clipboard';
import { memo, useEffect, useMemo, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';
import Markdown, { MarkdownIt, type RenderRules } from 'react-native-markdown-display';

import { fileLabel, splitAttached } from '@/chat/attach';
import { fromCwd, questionLine, type Message } from '@/chat/model';
import { sizeLabel } from '@/chat/transcript';
import { tap } from '@/store/alerts';
import { agentColor, fonts, metrics, type Palette } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';

import { Callout, Label, Led, Txt } from './kit';

export function markdownStyles(t: Palette) {
  const code = {
    fontFamily: fonts.mono,
    fontSize: metrics.textCode,
    lineHeight: 19,
    color: t.text,
    backgroundColor: t.surface2,
    borderColor: t.border,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radiusCell,
  };
  return StyleSheet.create({
    body: { fontFamily: fonts.sans, fontSize: metrics.textBody, lineHeight: 23, color: t.text },
    paragraph: { marginTop: 0, marginBottom: 10 },
    heading1: { fontFamily: fonts.sansSemibold, fontSize: 21, lineHeight: 28, marginBottom: 8, letterSpacing: -0.4 },
    heading2: { fontFamily: fonts.sansSemibold, fontSize: 19, lineHeight: 26, marginBottom: 8, letterSpacing: -0.3 },
    heading3: { fontFamily: fonts.sansSemibold, fontSize: 17, lineHeight: 24, marginBottom: 6 },
    strong: { fontFamily: fonts.sansSemibold, fontWeight: 'normal' },
    link: { color: t.signal, textDecorationLine: 'underline' },
    blockquote: {
      backgroundColor: 'transparent',
      borderLeftColor: t.gridMark,
      borderLeftWidth: 2,
      paddingLeft: 12,
      marginLeft: 0,
    },
    // Inside a line of prose it takes that line's height: a height of its
    // own makes its background clip the lines around it.
    code_inline: {
      fontFamily: fonts.mono,
      fontSize: metrics.textCode,
      color: t.text,
      backgroundColor: t.surface2,
    },
    code_block: { ...code, padding: 12, marginBottom: 10 },
    fence: { ...code, padding: 12, marginBottom: 10 },
    hr: { backgroundColor: t.border, height: metrics.hairline },
    table: { borderColor: t.border, borderRadius: metrics.radiusCell },
    tr: { borderColor: t.border },
    bullet_list_icon: { color: t.textFaint },
    ordered_list_icon: { color: t.textFaint, fontFamily: fonts.mono, fontSize: metrics.textSmall },
  });
}

export function openLink(url: string): boolean {
  if (/^https?:\/\//i.test(url)) void Linking.openURL(url);
  return false;
}

/**
 * How an agent's Markdown is read: with bare addresses (`https://…`,
 * `www.…`) made links, as agents seldom write them as Markdown links.
 */
export const markdownParser = MarkdownIt({ typographer: true, linkify: true });

/** Its text can be selected, a word or a passage, as any text of the phone. */
export const markdownRules: RenderRules = {
  textgroup: (node, children, _parent, styles) => (
    <Text key={node.key} selectable style={styles.textgroup}>
      {children}
    </Text>
  ),
};

/** The button that copies a whole message; it shows a tick for a moment after. */
export function CopyButton({ text, what }: { text: string; what: string }) {
  const t = useTheme();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1_500);
    return () => clearTimeout(timer);
  }, [copied]);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={copied ? 'Copied' : `Copy ${what}`}
      hitSlop={10}
      onPress={() => {
        tap();
        void Clipboard.setStringAsync(text).then(() => setCopied(true), () => undefined);
      }}
      style={({ pressed }) => [styles.copy, { opacity: pressed ? 0.6 : 1 }]}>
      <Feather name={copied ? 'check' : 'copy'} size={14} color={copied ? t.success : t.textFaint} />
    </Pressable>
  );
}

/** Holding a tool's line copies what it holds: the phone says so with a tap. */
function copy(text: string): void {
  tap();
  void Clipboard.setStringAsync(text).catch(() => undefined);
}

/**
 * Text as a terminal would print it. The lines of a diff take the colour of
 * what they did, with their sign in front: never colour alone.
 */
function Block({ text, diff = false }: { text: string; diff?: boolean }) {
  const t = useTheme();
  return (
    <Text
      selectable
      maxFontSizeMultiplier={1.5}
      style={[styles.block, { color: t.textMuted, backgroundColor: t.surface2, borderColor: t.border }]}>
      {diff
        ? text.split('\n').map((line, at) => (
            <Text
              key={at}
              style={{
                color: /^\+(?!\+\+)/.test(line)
                  ? t.success
                  : /^-(?!--)/.test(line)
                    ? t.error
                    : /^@@/.test(line)
                      ? t.info
                      : t.textMuted,
              }}>
              {at > 0 ? '\n' : ''}
              {line}
            </Text>
          ))
        : text}
    </Text>
  );
}

type Props = {
  message: Message;
  /** The agent of the chat: its catalogue tag and the name it goes by. */
  agent: string;
  agentName: string;
  /** The folder the chat works in: paths inside it are said from there. */
  cwd?: string;
  /** A note or a tool shows what it holds: the chat keeps which ones are open. */
  open?: boolean;
  onToggle?(id: string): void;
};

/** What the person said: the words, and the files that went with them as chips. */
function UserText({ text }: { text: string }) {
  const t = useTheme();
  const said = useMemo(() => splitAttached(text), [text]);
  return (
    <>
      {said.text ? <Txt selectable>{said.text}</Txt> : null}
      {said.files.length > 0 ? (
        <View style={styles.files}>
          {said.files.map((path) => (
            <View
              key={path}
              accessibilityLabel={`Attached ${fileLabel(path)}`}
              style={[styles.file, { borderColor: t.border, backgroundColor: t.background }]}>
              <Feather name="paperclip" size={12} color={t.textMuted} />
              <Label tone="muted" numberOfLines={1} ellipsizeMode="middle" style={styles.fileName}>
                {fileLabel(path)}
              </Label>
            </View>
          ))}
        </View>
      ) : null}
    </>
  );
}

function MessageRowInner({ message, agent, agentName, cwd, open = false, onToggle }: Props) {
  const t = useTheme();
  const md = useMemo(() => markdownStyles(t), [t]);

  switch (message.kind) {
    case 'user':
      return (
        <View
          style={[styles.user, { backgroundColor: t.surface, borderColor: t.border, borderLeftColor: t.signal }]}>
          <View style={styles.who}>
            <Txt variant="mono" tone="signal">
              {'>'}
            </Txt>
            <Label tone="muted" style={{ flex: 1 }}>
              You
            </Label>
            <CopyButton text={message.text} what="what you asked" />
          </View>
          <UserText text={message.text} />
        </View>
      );

    case 'assistant':
      return (
        <View style={styles.assistant}>
          <View style={styles.who}>
            <Led color={agentColor(t, agent)} />
            <Label style={{ color: agentColor(t, agent), flex: 1 }}>{agentName}</Label>
            {message.streaming ? null : <CopyButton text={message.text} what="the answer" />}
          </View>
          <Markdown style={md} onLinkPress={openLink} markdownit={markdownParser} rules={markdownRules}>
            {message.text}
          </Markdown>
          {message.streaming ? (
            <View
              accessibilityLabel="Still writing"
              style={{ width: 8, height: 16, backgroundColor: t.signal }}
            />
          ) : null}
        </View>
      );

    case 'tool': {
      // A state is a glyph and a colour, as a line of a terminal would say it.
      const glyph =
        message.status === 'ok'
          ? '✓'
          : message.status === 'error'
            ? '✗'
            : message.status === 'running'
              ? '…'
              : '–';
      const color =
        message.status === 'ok'
          ? t.success
          : message.status === 'error'
            ? t.error
            : message.status === 'running'
              ? t.signal
              : t.textFaint;
      const said =
        message.status === 'running'
          ? 'running'
          : message.status === 'ok'
            ? 'done'
            : message.status === 'error'
              ? 'failed'
              : 'stopped';
      // More than its line says: the whole command, the edit, what came back.
      const more =
        Boolean(message.detail || message.output) ||
        message.summary.length > 34 ||
        Boolean(message.result && message.status !== 'error');
      const failure = message.status === 'error' ? (message.output ?? message.result) : undefined;
      return (
        <View style={[styles.tool, { borderLeftColor: open ? t.gridMark : t.guide }]}>
          <Pressable
            accessibilityRole="button"
            accessibilityState={more ? { expanded: open } : undefined}
            accessibilityLabel={`${message.name} ${message.summary}, ${said}`}
            accessibilityHint={more ? (open ? 'Hides what it did.' : 'Shows what it did.') : undefined}
            disabled={!more}
            onPress={() => onToggle?.(message.id)}
            onLongPress={() => copy(message.detail ?? message.summary)}
            style={({ pressed }) => [styles.toolHead, { backgroundColor: pressed ? t.surface2 : 'transparent' }]}>
            <Txt variant="mono" style={{ color, width: 12 }}>
              {glyph}
            </Txt>
            <Label tone={open ? 'text' : 'muted'}>{message.name}</Label>
            {message.summary ? (
              <Txt
                variant="mono"
                tone="faint"
                numberOfLines={1}
                ellipsizeMode="middle"
                style={{ flex: 1 }}>
                {fromCwd(message.summary, cwd)}
              </Txt>
            ) : (
              <View style={{ flex: 1 }} />
            )}
            {more ? (
              <Txt variant="mono" tone="faint" style={{ width: 14, textAlign: 'right' }}>
                {open ? '▾' : '▸'}
              </Txt>
            ) : null}
          </Pressable>
          {open ? (
            <View style={styles.toolBody}>
              <Block text={fromCwd(message.detail ?? message.summary, cwd)} diff={Boolean(message.detail)} />
              {message.output ?? message.result ? (
                <>
                  <Label tone={message.status === 'error' ? 'error' : 'faint'}>
                    {message.status === 'error' ? 'Failed' : 'Returned'}
                  </Label>
                  <Block text={(message.output ?? message.result)!} />
                </>
              ) : null}
            </View>
          ) : failure ? (
            <Txt variant="mono" tone="error" numberOfLines={4} style={{ paddingLeft: 18 }}>
              {failure}
            </Txt>
          ) : null}
        </View>
      );
    }

    case 'note':
      // What a program wrote into the conversation: one line until opened.
      return (
        <View style={[styles.note, { borderLeftColor: t.guide }]}>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ expanded: open }}
            accessibilityLabel={`System note: ${message.label}, ${sizeLabel(message.text)}`}
            accessibilityHint={open ? 'Hides its text.' : 'Shows its text.'}
            onPress={() => onToggle?.(message.id)}
            style={({ pressed }) => [styles.noteHead, { backgroundColor: pressed ? t.surface2 : 'transparent' }]}>
            <Txt variant="mono" tone="faint" style={{ width: 12 }}>
              {open ? '▾' : '▸'}
            </Txt>
            <Label tone="faint">System</Label>
            <Txt variant="mono" tone="faint" numberOfLines={1} style={{ flex: 1 }}>
              {message.label} · {sizeLabel(message.text)}
            </Txt>
          </Pressable>
          {open ? (
            <Txt
              variant="mono"
              tone="muted"
              selectable
              style={[styles.noteText, { backgroundColor: t.surface2, borderColor: t.border }]}>
              {message.text}
            </Txt>
          ) : null}
        </View>
      );

    case 'notice':
      return (
        <Callout tone={message.tone} style={styles.notice}>
          <Txt variant="small" tone="muted" selectable>
            {message.text}
          </Txt>
        </Callout>
      );

    case 'question': {
      // The record of something the agent asked: a line, as a tool is. The
      // question itself is answered above the composer while it is open.
      const [glyph, color, word] =
        message.status === 'open'
          ? ['?', t.warning, 'Waiting for you']
          : message.status === 'sending'
            ? ['…', t.signal, 'Answer on its way']
            : message.status === 'answered'
              ? ['✓', t.success, 'Answered']
              : ['–', t.textFaint, 'Not answered here'];
      const asked = fromCwd(questionLine(message.ask), cwd);
      return (
        <View
          accessible
          accessibilityLabel={`${word}. ${asked}${message.chosen ? `. ${message.chosen}` : ''}`}
          style={[styles.tool, { borderLeftColor: color }]}>
          <View style={styles.toolHead}>
            <Txt variant="mono" style={{ color, width: 12 }}>
              {glyph}
            </Txt>
            <Label style={{ color }}>{word}</Label>
            <Txt variant="mono" tone="muted" numberOfLines={1} style={{ flex: 1, fontSize: 12 }}>
              {asked}
            </Txt>
          </View>
          {message.chosen ? (
            <Txt variant="mono" tone="text" numberOfLines={3} style={{ fontSize: 12, paddingLeft: 18 }}>
              {message.chosen}
            </Txt>
          ) : null}
        </View>
      );
    }
  }
}

export const MessageRow = memo(MessageRowInner);

const styles = StyleSheet.create({
  files: { flexDirection: 'row', flexWrap: 'wrap', gap: 6, marginTop: 8 },
  file: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    maxWidth: '100%',
    minHeight: 28,
    paddingHorizontal: 8,
    borderWidth: 1,
    borderRadius: metrics.radius,
  },
  fileName: { flexShrink: 1, textTransform: 'none', letterSpacing: 0 },
  user: {
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
  copy: { paddingHorizontal: 4, paddingVertical: 2 },
  assistant: { paddingHorizontal: metrics.pad, paddingTop: 4 },
  tool: {
    marginHorizontal: metrics.pad,
    marginVertical: 2,
    paddingLeft: 10,
    borderLeftWidth: 1,
    gap: 2,
  },
  toolHead: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 30, paddingRight: 4 },
  toolBody: { gap: 6, paddingTop: 2, paddingBottom: 8 },
  block: {
    padding: 10,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radiusCell,
    fontFamily: fonts.mono,
    fontSize: 12,
    lineHeight: 17,
  },
  notice: { marginHorizontal: metrics.pad, marginVertical: 8 },
  note: { marginHorizontal: metrics.pad, marginVertical: 2, paddingLeft: 10, borderLeftWidth: 1 },
  noteHead: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 32 },
  noteText: {
    marginTop: 2,
    marginBottom: 6,
    padding: 10,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radiusCell,
    fontSize: 12,
    lineHeight: 17,
  },
});
