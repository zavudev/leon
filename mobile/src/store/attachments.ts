// The files waiting to go with the next message of each chat: a photo, a
// screenshot pasted from the clipboard, a document. Each is sent to the
// computer as soon as it is picked, so that sending the message only names
// the paths it was kept at; until then its chip says it is on its way.
// Kept while the application lives, as the drafts are.
import { create } from 'zustand';

import { MAX_ATTACHMENT_BYTES, weight } from '@/chat/attach';
import { explain } from '@/link/errors';

import { useApp } from './app';
import { ensureLink } from './runtime';

/** One file attached to the next message of a chat. */
export type Attachment = {
  id: string;
  name: string;
  /** An image the chip can show, as a URI the phone can draw. */
  preview?: string;
  state: 'sending' | 'kept' | 'failed';
  /** Where it was kept on the computer, once it is. */
  path?: string;
  /** Why it could not be sent. */
  problem?: string;
};

/** What was picked: its name, and how to read it when it is sent. */
export type Picked = {
  name: string;
  size?: number | null;
  preview?: string;
  read(): Promise<ArrayBuffer>;
};

export const useAttachments = create<{ byChat: Record<string, Attachment[]> }>(() => ({ byChat: {} }));

let counter = 0;

function patch(chatId: string, id: string, change: Partial<Attachment>): void {
  useAttachments.setState((s) => {
    const list = s.byChat[chatId];
    if (!list?.some((a) => a.id === id)) return s;
    return { byChat: { ...s.byChat, [chatId]: list.map((a) => (a.id === id ? { ...a, ...change } : a)) } };
  });
}

/** Attaches `picked` to the next message of `chatId` and sends it to the chat's computer. */
export async function attach(chatId: string, picked: Picked): Promise<void> {
  const chat = useApp.getState().chats[chatId];
  if (!chat) return;
  const id = `${Date.now().toString(36)}-${(counter++).toString(36)}`;
  useAttachments.setState((s) => ({
    byChat: {
      ...s.byChat,
      [chatId]: [...(s.byChat[chatId] ?? []), { id, name: picked.name, preview: picked.preview, state: 'sending' }],
    },
  }));
  const tooBig = (size: number) => `${weight(size)}: larger than ${weight(MAX_ATTACHMENT_BYTES)}`;
  if (picked.size && picked.size > MAX_ATTACHMENT_BYTES) {
    patch(chatId, id, { state: 'failed', problem: tooBig(picked.size) });
    return;
  }
  try {
    const bytes = await picked.read();
    if (bytes.byteLength > MAX_ATTACHMENT_BYTES) {
      patch(chatId, id, { state: 'failed', problem: tooBig(bytes.byteLength) });
      return;
    }
    const host = ensureLink(chat.hostId);
    if (!host) throw new Error('That computer is no longer paired with this device.');
    const path = await host.keepAttachment(chatId, picked.name, bytes);
    patch(chatId, id, { state: 'kept', path });
  } catch (error) {
    const said = explain(error);
    patch(chatId, id, { state: 'failed', problem: said.headline });
  }
}

/** Takes a file off the next message. What was sent stays on the computer, unread. */
export function dropAttachment(chatId: string, id: string): void {
  useAttachments.setState((s) => {
    const list = (s.byChat[chatId] ?? []).filter((a) => a.id !== id);
    const byChat = { ...s.byChat };
    if (list.length > 0) byChat[chatId] = list;
    else delete byChat[chatId];
    return { byChat };
  });
}

/** The paths the next message of `chatId` names, and clears them: it is being sent. */
export function takeAttached(chatId: string): string[] {
  const list = useAttachments.getState().byChat[chatId] ?? [];
  const paths = list.flatMap((a) => (a.state === 'kept' && a.path ? [a.path] : []));
  dropAll(chatId);
  return paths;
}

/** Forgets what was attached to a chat. */
export function dropAll(chatId: string): void {
  useAttachments.setState((s) => {
    if (!(chatId in s.byChat)) return s;
    const byChat = { ...s.byChat };
    delete byChat[chatId];
    return { byChat };
  });
}
