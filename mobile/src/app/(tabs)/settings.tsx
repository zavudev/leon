// Settings: how this device names itself, how it looks, whether King's
// notices reach it while closed, the relay it offers by default.
import { useMemo, useState } from 'react';
import { Linking, ScrollView, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';

import { explain } from '@/link/errors';
import { DEFAULT_RELAY_URL, useApp, type Appearance } from '@/store/app';
import { disableKingPush, enableKingPush } from '@/store/push';
import { link } from '@/store/runtime';
import { Button, Chip, Field, Label, Panel, Screen, Section, Signature, Txt } from '@/ui/kit';
import { Mark } from '@/ui/Mark';

const REPOSITORY = 'https://github.com/zavudev/leon';

const APPEARANCES: { id: Appearance; name: string }[] = [
  { id: 'dark', name: 'Dark' },
  { id: 'light', name: 'Light' },
  { id: 'system', name: 'System' },
];

const PLAIN = { paddingHorizontal: 0, paddingTop: 8, paddingBottom: 0, minHeight: 0 } as const;

export default function Settings() {
  const deviceName = useApp((s) => s.deviceName);
  const relayUrl = useApp((s) => s.relayUrl);
  const appearance = useApp((s) => s.appearance);
  const kingPush = useApp((s) => s.kingPush);
  const [pushBusy, setPushBusy] = useState(false);
  const [pushError, setPushError] = useState<string | null>(null);
  const [name, setName] = useState(deviceName);
  const [relay, setRelay] = useState(relayUrl);
  const deviceId = useMemo(() => {
    try {
      return link().deviceId();
    } catch (error) {
      return explain(error).headline;
    }
  }, []);

  function saveName() {
    useApp.getState().setDeviceName(name);
    const saved = useApp.getState().deviceName;
    setName(saved);
    link().setDeviceName(saved);
  }

  function setKingPush(on: boolean) {
    if (on === kingPush || pushBusy) return;
    setPushError(null);
    if (!on) {
      disableKingPush();
      return;
    }
    setPushBusy(true);
    void enableKingPush()
      .then((why) => setPushError(why ?? null))
      .finally(() => setPushBusy(false));
  }

  function saveRelay() {
    useApp.getState().setRelayUrl(relay);
    setRelay(useApp.getState().relayUrl);
  }

  return (
    <Screen kind="Leon" title="Settings" back={false} bottomInset={false}>
      <KeyboardAvoidingView behavior="padding" style={{ flex: 1 }}>
        <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={{ padding: 16, gap: 16 }}>
          <Section title="This device" style={PLAIN} />
          <View style={{ gap: 6 }}>
            <Field
              label="Name of this device"
              value={name}
              onChangeText={setName}
              onEndEditing={saveName}
              maxLength={64}
              returnKeyType="done"
            />
            <Txt variant="small" tone="faint">
              What a computer shows when this device asks to pair. It applies to the computers you
              connect from now on.
            </Txt>
          </View>

          <Panel style={{ gap: 6 }}>
            <Label>Device id</Label>
            <Txt variant="mono" selectable style={{ fontSize: 15, lineHeight: 22 }}>
              {deviceId}
            </Txt>
            <Txt variant="small" tone="faint">
              The fingerprint of this device&apos;s key. A computer lists its paired devices by it,
              and that is where you revoke one.
            </Txt>
          </Panel>

          <Section title="Appearance" style={PLAIN} />
          <View style={{ gap: 6 }}>
            <View accessibilityRole="radiogroup" style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 8 }}>
              {APPEARANCES.map((choice) => (
                <Chip
                  key={choice.id}
                  title={choice.name}
                  selected={appearance === choice.id}
                  onPress={() => useApp.getState().setAppearance(choice.id)}
                />
              ))}
            </View>
            <Txt variant="small" tone="faint">
              Leon is dark unless you say otherwise. System follows this phone.
            </Txt>
          </View>

          <Section title="King's notices while closed" style={PLAIN} />
          <View style={{ gap: 6 }}>
            <View accessibilityRole="radiogroup" style={{ flexDirection: 'row', gap: 8 }}>
              <Chip title="Off" selected={!kingPush} onPress={() => setKingPush(false)} />
              <Chip
                title={pushBusy ? 'Turning on…' : 'On'}
                selected={kingPush}
                disabled={pushBusy}
                onPress={() => setKingPush(true)}
              />
            </View>
            <Txt variant="small" tone="faint">
              When King tells you something and Leon is closed, the title and body of its notice go
              through Expo&apos;s push service to reach this phone.
            </Txt>
            {pushError ? (
              <Txt variant="small" tone="error">
                {pushError}
              </Txt>
            ) : null}
          </View>

          <Section title="Relay" style={PLAIN} />

          <View style={{ gap: 6 }}>
            <Field
              label="Relay server"
              mono
              value={relay}
              onChangeText={setRelay}
              onEndEditing={saveRelay}
              autoCapitalize="none"
              keyboardType="url"
              placeholder={DEFAULT_RELAY_URL}
              returnKeyType="done"
            />
            <Txt variant="small" tone="faint">
              The relay offered when you connect a computer. Computers already connected keep the
              relay they were paired through.
            </Txt>
            {relay !== DEFAULT_RELAY_URL ? (
              <Button
                title="Use the default relay"
                onPress={() => {
                  useApp.getState().setRelayUrl(DEFAULT_RELAY_URL);
                  setRelay(DEFAULT_RELAY_URL);
                }}
                compact
              />
            ) : null}
          </View>

          <Section title="About" style={PLAIN} />
          <View style={{ gap: 10 }}>
            <View style={{ flexDirection: 'row', alignItems: 'center', gap: 12 }}>
              <Mark size={40} />
              <Signature style={{ textAlign: 'left' }} />
            </View>
            <Txt variant="small" tone="muted">
              A paired device can run any command on the computer as you. Pair only your own
              devices, and revoke a lost one from the computer.
            </Txt>
            <Txt variant="small" tone="faint">
              Open source under the Apache License 2.0. Inter and JetBrains Mono are under the SIL
              Open Font License. “Leon” and the lion mark are trademarks of Zavu.
            </Txt>
            <Button
              title="Source and licences"
              icon="external-link"
              onPress={() => void Linking.openURL(REPOSITORY)}
              compact
            />
          </View>
        </ScrollView>
      </KeyboardAvoidingView>
    </Screen>
  );
}
