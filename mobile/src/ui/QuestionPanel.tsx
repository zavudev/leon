// What the agent asked and waits for, above the composer: nobody should have
// to scroll to find what holds the agent up. One question at a time; the
// rest wait their turn behind it.
import { useMemo, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, TextInput, View, useWindowDimensions } from 'react-native';
import Markdown from 'react-native-markdown-display';

import type { QuestionMessage } from '@/chat/model';
import type { Answer, ChoiceQuestion, PermissionChoice, QuestionKind } from '@/link/types';
import { fonts, metrics } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';

import { Button, Label, Led, Txt } from './kit';
import { markdownParser, markdownRules, markdownStyles, openLink } from './MessageRow';

type Props = {
  /** The questions still pending, oldest first; the first one is shown. */
  questions: QuestionMessage[];
  agentName: string;
  online: boolean;
  onAnswer(questionId: string, answer: Answer): void;
};

/** A one-line text field in the panel's own style. */
function Line({
  label,
  value,
  onChangeText,
  placeholder,
}: {
  label: string;
  value: string;
  onChangeText(text: string): void;
  placeholder: string;
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
      multiline
      style={[styles.line, { color: t.text, borderColor: t.border, backgroundColor: t.background }]}
    />
  );
}

function PermissionBody({
  ask,
  onAnswer,
}: {
  ask: Extract<QuestionKind, { kind: 'permission' }>;
  onAnswer(answer: Answer): void;
}) {
  const t = useTheme();
  const [detail, setDetail] = useState(false);
  // The refusal a reason is being written for, by its id.
  const [refusing, setRefusing] = useState<string | null>(null);
  const [reason, setReason] = useState('');
  const allow = ask.choices.filter((c) => c.effect !== 'deny');
  // An agent may offer several ways to refuse (deny this, or deny and stop);
  // only some take a reason.
  const refusals = ask.choices.filter((c) => c.effect === 'deny');
  const choose = (choice: PermissionChoice) => onAnswer({ kind: 'permission', choiceId: choice.id });
  return (
    <View style={{ gap: 10 }}>
      <View style={{ gap: 2 }}>
        <Label tone="text">{ask.tool}</Label>
        {ask.summary ? (
          <Txt variant="mono" selectable style={{ fontSize: 13, lineHeight: 19 }}>
            {ask.summary}
          </Txt>
        ) : null}
        {ask.reason && ask.reason !== ask.summary ? (
          <Txt variant="small" tone="muted">
            {ask.reason}
          </Txt>
        ) : null}
      </View>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ expanded: detail }}
        accessibilityLabel="Everything it would pass to the tool"
        onPress={() => setDetail((on) => !on)}
        style={{ minHeight: 32, flexDirection: 'row', alignItems: 'center', gap: 6 }}>
        <Txt variant="mono" tone="faint">
          {detail ? '▾' : '▸'}
        </Txt>
        <Label>Detail</Label>
      </Pressable>
      {detail ? (
        <Txt
          variant="mono"
          selectable
          tone="muted"
          style={[styles.detail, { borderColor: t.border, backgroundColor: t.surface2 }]}>
          {ask.detail}
        </Txt>
      ) : null}
      {allow.map((choice, index) => (
        <View key={choice.id} style={{ gap: 3 }}>
          <Button
            kind={index === 0 ? 'primary' : 'outline'}
            title={choice.label}
            onPress={() => choose(choice)}
          />
          {choice.scope ? (
            <Label style={{ textTransform: 'none', letterSpacing: 0 }}>For {choice.scope}.</Label>
          ) : null}
        </View>
      ))}
      {refusals.map((deny) =>
        refusing === deny.id ? (
          <View key={deny.id} style={{ gap: 8 }}>
            <Line
              label="Why not"
              value={reason}
              onChangeText={setReason}
              placeholder="Why not, or what to do instead (optional)"
            />
            <Button
              kind="danger"
              title={deny.label}
              onPress={() =>
                onAnswer({ kind: 'permission', choiceId: deny.id, reason: reason.trim() || undefined })
              }
            />
          </View>
        ) : deny.takesReason ? (
          <View key={deny.id} style={{ flexDirection: 'row', gap: 8 }}>
            <Button style={{ flex: 1 }} title={deny.label} onPress={() => choose(deny)} />
            <Button
              style={{ flex: 1 }}
              title="Deny, and say why"
              onPress={() => {
                setReason('');
                setRefusing(deny.id);
              }}
            />
          </View>
        ) : (
          <Button key={deny.id} title={deny.label} onPress={() => choose(deny)} />
        ),
      )}
      {refusals.length > 0 && refusals.every((deny) => !deny.takesReason) ? (
        <Label style={{ textTransform: 'none', letterSpacing: 0 }}>
          To say what to do instead, deny and write it as your next message.
        </Label>
      ) : null}
    </View>
  );
}

function ChoicesBody({
  ask,
  onAnswer,
}: {
  ask: Extract<QuestionKind, { kind: 'choices' }>;
  onAnswer(answer: Answer): void;
}) {
  const t = useTheme();
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});
  const answered = (q: ChoiceQuestion) =>
    (picked[q.id]?.length ?? 0) > 0 || (other[q.id]?.trim().length ?? 0) > 0;
  const complete = ask.questions.length > 0 && ask.questions.every(answered);

  function toggle(q: ChoiceQuestion, label: string) {
    setPicked((before) => {
      const had = before[q.id] ?? [];
      const next = q.multiSelect
        ? had.includes(label)
          ? had.filter((l) => l !== label)
          : [...had, label]
        : had.includes(label)
          ? []
          : [label];
      return { ...before, [q.id]: next };
    });
  }

  return (
    <View style={{ gap: 14 }}>
      {ask.questions.map((q) => (
        <View key={q.id} style={{ gap: 6 }}>
          {q.header ? <Label tone="muted">[ {q.header} ]</Label> : null}
          <Txt variant="strong">{q.question}</Txt>
          {q.multiSelect ? <Label>Pick any</Label> : null}
          <View accessibilityRole={q.multiSelect ? undefined : 'radiogroup'}>
            {q.options.map((option) => {
              const selected = picked[q.id]?.includes(option.label) ?? false;
              return (
                <Pressable
                  key={option.label}
                  accessibilityRole={q.multiSelect ? 'checkbox' : 'radio'}
                  accessibilityState={q.multiSelect ? { checked: selected } : { selected }}
                  accessibilityLabel={
                    option.description ? `${option.label}. ${option.description}` : option.label
                  }
                  onPress={() => toggle(q, option.label)}
                  style={({ pressed }) => [
                    styles.option,
                    {
                      borderColor: selected ? t.signal : t.border,
                      backgroundColor: pressed || selected ? t.surface2 : t.background,
                    },
                  ]}>
                  <View style={{ paddingTop: 5 }}>
                    <Led color={selected ? t.signal : t.textFaint} hollow={!selected} />
                  </View>
                  <View style={{ flex: 1, gap: 2 }}>
                    <Txt tone={selected ? 'signal' : 'text'}>{option.label}</Txt>
                    {option.description ? (
                      <Txt variant="small" tone="muted">
                        {option.description}
                      </Txt>
                    ) : null}
                  </View>
                </Pressable>
              );
            })}
          </View>
          {q.allowOther ? (
            <Line
              label={`Another answer to: ${q.question}`}
              value={other[q.id] ?? ''}
              onChangeText={(text) => setOther((before) => ({ ...before, [q.id]: text }))}
              placeholder="Or in your own words"
            />
          ) : null}
        </View>
      ))}
      <Button
        kind="primary"
        title="Answer"
        disabled={!complete}
        onPress={() =>
          onAnswer({
            kind: 'choices',
            answers: ask.questions.map((q) => ({
              questionId: q.id,
              labels: picked[q.id] ?? [],
              other: other[q.id]?.trim() || undefined,
            })),
          })
        }
      />
      <Button title="Skip the question" onPress={() => onAnswer({ kind: 'permission', choiceId: 'deny' })} />
    </View>
  );
}

function PlanBody({
  ask,
  onAnswer,
}: {
  ask: Extract<QuestionKind, { kind: 'plan' }>;
  onAnswer(answer: Answer): void;
}) {
  const t = useTheme();
  const md = useMemo(() => markdownStyles(t), [t]);
  const [feedback, setFeedback] = useState('');
  return (
    <View style={{ gap: 10 }}>
      <View style={[styles.plan, { borderColor: t.border, backgroundColor: t.background }]}>
        {ask.plan.trim() ? (
          <Markdown style={md} onLinkPress={openLink} markdownit={markdownParser} rules={markdownRules}>
            {ask.plan}
          </Markdown>
        ) : (
          <Txt variant="small" tone="muted">
            The agent gave no text of its plan: it is in what it wrote above.
          </Txt>
        )}
      </View>
      <Button kind="primary" title="Approve, start working" onPress={() => onAnswer({ kind: 'plan', approve: true })} />
      <Line
        label="What to change in the plan"
        value={feedback}
        onChangeText={setFeedback}
        placeholder="What to change (optional)"
      />
      <Button
        title="Keep planning"
        onPress={() => onAnswer({ kind: 'plan', approve: false, feedback: feedback.trim() || undefined })}
      />
    </View>
  );
}

export function QuestionPanel({ questions, agentName, online, onAnswer }: Props) {
  const t = useTheme();
  const { height } = useWindowDimensions();
  const current = questions[0];
  if (!current) return null;
  const answer = (reply: Answer) => onAnswer(current.questionId, reply);
  const sending = current.status === 'sending';
  const title =
    current.ask.kind === 'permission'
      ? `${agentName} asks to use a tool`
      : current.ask.kind === 'plan'
        ? `${agentName} has a plan`
        : `${agentName} asks`;
  return (
    <View
      accessibilityLiveRegion="polite"
      style={{ borderTopWidth: 1, borderTopColor: sending ? t.signal : t.warning, backgroundColor: t.surface }}>
      <View style={styles.head}>
        <Label tone={sending ? 'signal' : 'warning'}>
          [ {sending ? 'Answer on its way' : 'Waiting for you'} ]
        </Label>
        <Label tone="muted" numberOfLines={1} style={{ flex: 1, textTransform: 'none', letterSpacing: 0 }}>
          {title}
        </Label>
        {questions.length > 1 ? <Label>1 of {questions.length}</Label> : null}
      </View>
      <ScrollView
        keyboardShouldPersistTaps="handled"
        style={{ maxHeight: height * 0.5 }}
        contentContainerStyle={{ paddingHorizontal: metrics.pad, paddingBottom: 14 }}>
        {sending ? (
          <View style={{ gap: 6 }}>
            <Txt variant="mono" style={{ fontSize: 13, lineHeight: 19 }}>
              {current.chosen}
            </Txt>
            <Txt variant="small" tone="muted">
              {online
                ? `${agentName} has not read it yet.`
                : 'Not connected: it is kept, and sent as soon as the connection is back.'}
            </Txt>
          </View>
        ) : current.ask.kind === 'permission' ? (
          <PermissionBody key={current.id} ask={current.ask} onAnswer={answer} />
        ) : current.ask.kind === 'choices' ? (
          <ChoicesBody key={current.id} ask={current.ask} onAnswer={answer} />
        ) : (
          <PlanBody key={current.id} ask={current.ask} onAnswer={answer} />
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  head: {
    minHeight: metrics.touch,
    paddingHorizontal: metrics.pad,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  line: {
    minHeight: metrics.touch,
    maxHeight: 96,
    paddingHorizontal: 10,
    paddingVertical: 9,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radius,
    fontFamily: fonts.sans,
    fontSize: metrics.textSmall,
    lineHeight: 19,
  },
  detail: {
    padding: 10,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radiusCell,
    fontSize: 12,
    lineHeight: 17,
  },
  option: {
    flexDirection: 'row',
    gap: 10,
    minHeight: metrics.touch,
    paddingHorizontal: 10,
    paddingVertical: 9,
    marginBottom: 4,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radiusCell,
  },
  plan: {
    paddingHorizontal: 12,
    paddingVertical: 4,
    borderWidth: metrics.hairline,
    borderRadius: metrics.radiusCell,
  },
});
