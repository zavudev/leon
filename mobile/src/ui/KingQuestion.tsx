// What King or one of its workers asked and waits for, above the line at the
// bottom: its question, a button per answer (allow, always, deny; an option;
// approve or reject a plan), and a field for words. One question at a time;
// the rest wait behind it. The chats' `QuestionPanel` does not fit: King's
// service gives a question as text and how it may be answered, no more.
import { useState } from 'react';
import { ScrollView, StyleSheet, TextInput, View, useWindowDimensions } from 'react-native';

import { picksOf, takesWords, wordsHint, type KingQuestion, type KingReply } from '@/king/model';
import { tap } from '@/store/alerts';
import { fonts, metrics } from '@/theme/tokens';
import { useTheme } from '@/theme/useTheme';

import { Button, Label, Txt } from './kit';

type Props = {
  /** The questions waiting, oldest first; the first one is shown. */
  questions: KingQuestion[];
  /** Who asks: `King`, `Worker #2`. */
  who: string;
  online: boolean;
  /** Answers the question with the words written; resolves once it is done or refused. */
  onAnswer(question: string, reply: KingReply, words: string): Promise<boolean>;
};

export function KingQuestionPanel({ questions, who, online, onAnswer }: Props) {
  const t = useTheme();
  const { height } = useWindowDimensions();
  const [words, setWords] = useState('');
  const [sending, setSending] = useState(false);
  const current = questions[0];
  if (!current) return null;
  const picks = picksOf(current);
  const answer = (reply: KingReply) => {
    tap();
    setSending(true);
    void onAnswer(current.id, reply, words).then((done) => {
      setSending(false);
      if (done) setWords('');
    });
  };
  const wordsAlone = takesWords(current);
  return (
    <View
      accessibilityLiveRegion="polite"
      style={{ borderTopWidth: 1, borderTopColor: t.warning, backgroundColor: t.surface }}>
      <View style={styles.head}>
        <Label tone="warning">[ Waiting for you ]</Label>
        <Label tone="muted" numberOfLines={1} style={{ flex: 1, textTransform: 'none', letterSpacing: 0 }}>
          {who} asks
        </Label>
        {questions.length > 1 ? <Label>1 of {questions.length}</Label> : null}
      </View>
      <ScrollView
        keyboardShouldPersistTaps="handled"
        style={{ maxHeight: height * 0.45 }}
        contentContainerStyle={{ paddingHorizontal: metrics.pad, paddingBottom: 14, gap: 10 }}>
        <Txt variant="small" selectable>
          {current.text}
        </Txt>
        {picks.length > 0 ? (
          <View style={styles.picks}>
            {picks.map((pick, index) => (
              <Button
                key={`${index}-${pick.label}`}
                compact
                kind={pick.tone === 'stop' ? 'danger' : index === 0 && pick.tone === 'go' ? 'primary' : 'outline'}
                title={pick.label}
                disabled={!online || sending}
                onPress={() => answer(pick.reply)}
              />
            ))}
          </View>
        ) : null}
        <TextInput
          accessibilityLabel={wordsHint(current)}
          value={words}
          onChangeText={setWords}
          placeholder={wordsHint(current)}
          placeholderTextColor={t.textFaint}
          selectionColor={t.signal}
          cursorColor={t.signal}
          multiline
          editable={!sending}
          style={[styles.line, { color: t.text, borderColor: t.border, backgroundColor: t.background }]}
        />
        {wordsAlone ? (
          <Button
            title="Answer with what is written"
            busy={sending}
            disabled={!online || words.trim().length === 0}
            onPress={() => answer({})}
          />
        ) : null}
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
  picks: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
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
});
