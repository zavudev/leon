// Attaching files to a message: the sheet that picks them (a photo, the
// camera, a file, the image on the clipboard) and the row of what goes with
// the next message over the field, each sent to the computer as it is
// picked. A pasted image is the clipboard's: the row offers it as soon as
// the clipboard holds one.
import Feather from '@expo/vector-icons/Feather';
import * as Clipboard from 'expo-clipboard';
import * as DocumentPicker from 'expo-document-picker';
import { File } from 'expo-file-system';
import * as ImagePicker from 'expo-image-picker';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ActivityIndicator,
  Alert,
  AppState,
  Image,
  Modal,
  Platform,
  Pressable,
  ScrollView,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { bytesOfBase64, mimeOfDataUrl, nameOf } from '@/chat/attach';
import { attach, dropAttachment, useAttachments, type Attachment, type Picked } from '@/store/attachments';
import { metrics } from '@/theme/tokens';
import { useMotion, useTheme } from '@/theme/useTheme';

import { Label, Rule, Txt, type IconName } from './kit';

/** What a picked photo or file becomes: read from its URI when it is sent. */
function fromUri(uri: string, name: string, size?: number | null, image = false): Picked {
  return { name, size, preview: image ? uri : undefined, read: () => new File(uri).arrayBuffer() };
}

/** The image on the clipboard, as a file to attach; nothing when it holds none. */
async function clipboardImage(): Promise<Picked | null> {
  const image = await Clipboard.getImageAsync({ format: 'png' });
  if (!image?.data) return null;
  const data = image.data;
  return {
    name: nameOf(null, mimeOfDataUrl(data) ?? 'image/png', `pasted-${Date.now()}`),
    preview: data,
    read: async () => bytesOfBase64(data),
  };
}

/**
 * Whether the clipboard holds an image not pasted yet: asked when the field is
 * looked at, when the app comes back and when the clipboard changes. The
 * second value says it was pasted, so it is not offered twice.
 */
export function useClipboardImage(active: boolean): [boolean, () => void] {
  const [has, setHas] = useState(false);
  const check = useCallback(() => {
    Clipboard.hasImageAsync().then(setHas, () => setHas(false));
  }, []);
  useEffect(() => {
    if (!active) return;
    check();
    const subscription = AppState.addEventListener('change', (state) => {
      if (state === 'active') check();
    });
    const changed = Clipboard.addClipboardListener(check);
    return () => {
      subscription.remove();
      Clipboard.removeClipboardListener(changed);
    };
  }, [active, check]);
  const pasted = useCallback(() => setHas(false), []);
  return [has, pasted];
}

/** Pastes the clipboard's image into the next message of `chatId`. */
export async function pasteImage(chatId: string): Promise<void> {
  const picked = await clipboardImage();
  if (picked) void attach(chatId, picked);
}

type Way = { key: string; icon: IconName; title: string; detail: string; run(): Promise<void> };

/** Runs a way to attach; what went wrong is said over the chat, the sheet being gone. */
function start(way: Way): void {
  way.run().catch((error: unknown) =>
    Alert.alert('Nothing was attached', error instanceof Error ? error.message : String(error)),
  );
}

/** The sheet of the ways to attach a file. */
export function AttachSheet({
  chatId,
  visible,
  canPaste,
  onPasted,
  onClose,
}: {
  chatId: string;
  visible: boolean;
  canPaste: boolean;
  onPasted(): void;
  onClose(): void;
}) {
  const t = useTheme();
  const motion = useMotion();
  const insets = useSafeAreaInsets();
  // iOS shows one screen over the application at a time: the photos, the
  // camera or the files open once this sheet is gone, not while it closes.
  const pending = useRef<Way | null>(null);
  const choose = (way: Way) => {
    onClose();
    if (Platform.OS === 'ios') pending.current = way;
    else start(way);
  };

  const ways: Way[] = [
    {
      key: 'photos',
      icon: 'image',
      title: 'Photos',
      detail: 'Screenshots and pictures of this phone',
      async run() {
        const result = await ImagePicker.launchImageLibraryAsync({
          mediaTypes: ['images'],
          allowsMultipleSelection: true,
          selectionLimit: 6,
          quality: 0.8,
        });
        if (result.canceled) return;
        for (const asset of result.assets) {
          const name = nameOf(asset.fileName, asset.mimeType, `photo-${Date.now()}`);
          void attach(chatId, fromUri(asset.uri, name, asset.fileSize, true));
        }
      },
    },
    {
      key: 'camera',
      icon: 'camera',
      title: 'Camera',
      detail: 'Take a picture: a screen, a whiteboard, a sketch',
      async run() {
        const allowed = await ImagePicker.requestCameraPermissionsAsync();
        if (!allowed.granted) {
          throw new Error('Leon may not use the camera: allow it in the settings of this phone.');
        }
        const result = await ImagePicker.launchCameraAsync({ mediaTypes: ['images'], quality: 0.8 });
        if (result.canceled) return;
        const asset = result.assets[0];
        if (!asset) return;
        const name = nameOf(asset.fileName, asset.mimeType ?? 'image/jpeg', `camera-${Date.now()}`);
        void attach(chatId, fromUri(asset.uri, name, asset.fileSize, true));
      },
    },
    {
      key: 'file',
      icon: 'file',
      title: 'File',
      detail: 'A document, a log, a PDF: up to 20 MB',
      async run() {
        const result = await DocumentPicker.getDocumentAsync({ multiple: true, copyToCacheDirectory: true });
        if (result.canceled) return;
        for (const asset of result.assets) {
          const image = asset.mimeType?.startsWith('image/') ?? false;
          void attach(chatId, fromUri(asset.uri, nameOf(asset.name, asset.mimeType, 'file'), asset.size, image));
        }
      },
    },
  ];
  if (canPaste) {
    ways.push({
      key: 'paste',
      icon: 'clipboard',
      title: 'Paste image',
      detail: 'The image you copied',
      run: () => {
        onPasted();
        return pasteImage(chatId);
      },
    });
  }

  return (
    <Modal
      visible={visible}
      transparent
      animationType={motion.open === 0 ? 'none' : 'fade'}
      onRequestClose={onClose}
      onDismiss={() => {
        const way = pending.current;
        pending.current = null;
        if (way) start(way);
      }}>
      <Pressable
        accessibilityLabel="Close"
        accessibilityRole="button"
        onPress={onClose}
        style={{ flex: 1, backgroundColor: t.scrim }}
      />
      <View
        accessibilityViewIsModal
        style={{
          backgroundColor: t.surface,
          borderTopWidth: 1,
          borderTopColor: t.signal,
          paddingBottom: insets.bottom + 8,
        }}>
        <View style={{ padding: 16, gap: 4 }}>
          <Label tone="muted">[ Attach ]</Label>
          <Txt variant="small" tone="faint">
            It is sent to the computer now, and the agent opens it with your next message.
          </Txt>
        </View>
        <Rule />
        {ways.map((way) => (
          <Pressable
            key={way.key}
            accessibilityRole="button"
            accessibilityLabel={`${way.title}. ${way.detail}`}
            onPress={() => choose(way)}
            style={({ pressed }) => ({
              flexDirection: 'row',
              alignItems: 'center',
              gap: 14,
              minHeight: 56,
              paddingHorizontal: 16,
              borderBottomWidth: 1,
              borderBottomColor: t.border,
              backgroundColor: pressed ? t.surface2 : 'transparent',
            })}>
            <Feather name={way.icon} size={20} color={way.key === 'paste' ? t.signal : t.textMuted} />
            <View style={{ flex: 1, gap: 2 }}>
              <Txt variant="strong">{way.title}</Txt>
              <Txt variant="small" tone="faint">
                {way.detail}
              </Txt>
            </View>
          </Pressable>
        ))}
      </View>
    </Modal>
  );
}

function Chip({ chatId, item }: { chatId: string; item: Attachment }) {
  const t = useTheme();
  const failed = item.state === 'failed';
  return (
    <View
      accessibilityLabel={`${item.name}, ${failed ? `not sent: ${item.problem}` : item.state === 'sending' ? 'sending' : 'ready'}`}
      style={{
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        maxWidth: 220,
        minHeight: 40,
        paddingLeft: item.preview ? 4 : 10,
        borderWidth: 1,
        borderRadius: metrics.radius,
        borderColor: failed ? t.error : t.border,
        backgroundColor: t.background,
      }}>
      {item.preview ? (
        <Image source={{ uri: item.preview }} style={{ width: 32, height: 32, borderRadius: 3 }} />
      ) : (
        <Feather name="file" size={14} color={t.textMuted} />
      )}
      <View style={{ flexShrink: 1 }}>
        <Label tone={failed ? 'error' : 'text'} numberOfLines={1} ellipsizeMode="middle" style={{ textTransform: 'none', letterSpacing: 0 }}>
          {item.name}
        </Label>
        {failed ? (
          <Label tone="error" numberOfLines={1} style={{ textTransform: 'none', letterSpacing: 0 }}>
            {item.problem}
          </Label>
        ) : null}
      </View>
      {item.state === 'sending' ? <ActivityIndicator size="small" color={t.textMuted} /> : null}
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Remove ${item.name}`}
        hitSlop={6}
        onPress={() => dropAttachment(chatId, item.id)}
        style={({ pressed }) => ({ padding: 8, opacity: pressed ? 0.6 : 1 })}>
        <Feather name="x" size={14} color={t.textFaint} />
      </Pressable>
    </View>
  );
}

/**
 * What goes with the next message, over the field; with the image on the
 * clipboard offered first when there is one. Nothing when there is neither.
 */
export function AttachedRow({
  chatId,
  canPaste,
  onPasted,
}: {
  chatId: string;
  canPaste: boolean;
  onPasted(): void;
}) {
  const t = useTheme();
  const items = useAttachments((s) => s.byChat[chatId]);
  if (!items?.length && !canPaste) return null;
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      keyboardShouldPersistTaps="handled"
      contentContainerStyle={{ paddingHorizontal: metrics.pad, paddingTop: 8, gap: 8, alignItems: 'center' }}>
      {canPaste ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Paste the image you copied"
          onPress={() => {
            onPasted();
            void pasteImage(chatId);
          }}
          style={({ pressed }) => ({
            flexDirection: 'row',
            alignItems: 'center',
            gap: 8,
            minHeight: 40,
            paddingHorizontal: 12,
            borderWidth: 1,
            borderStyle: 'dashed',
            borderRadius: metrics.radius,
            borderColor: t.signal,
            backgroundColor: pressed ? t.surface2 : 'transparent',
          })}>
          <Feather name="clipboard" size={14} color={t.signal} />
          <Label tone="signal">Paste image</Label>
        </Pressable>
      ) : null}
      {items?.map((item) => <Chip key={item.id} chatId={chatId} item={item} />)}
    </ScrollView>
  );
}
