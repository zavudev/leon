// Telling the person that an agent needs them or is done: a tap of the motor
// when the chat is in front, and a line across the top on any other screen.
//
// Behind another application it is a system notification, on Android: a
// foreground service keeps the link while a turn runs (see `background.ts`),
// so the question or the end still arrives. iOS suspends the application and
// nothing arrives to be said; that needs the relay to push. There, and once
// Android has swiped the application away, what happened meanwhile is said
// when the person comes back: the link is reopened at once and the home lists
// who waits.
//
// King's notices take the same line, while its screen is open: they are only
// heard then.
import * as Haptics from 'expo-haptics';
import { router } from 'expo-router';
import * as Background from 'leon-background';
import { AppState } from 'react-native';
import { create } from 'zustand';

import { fromCwd } from '@/chat/model';

import { useApp } from './app';

/** What a chat's agent did; `king`: King of a computer told the person something. */
export type AlertKind = 'waiting' | 'done' | 'failed' | 'king';

/** The line, and what pressing it opens: a chat, or the King of a computer. */
export type Toast = { key: number; kind: AlertKind; title: string; line: string } & (
  | { chatId: string }
  | { kingOf: string }
);

/** The line across the top, while there is one. */
export const useAlerts = create<{ toast: Toast | null }>(() => ({ toast: null }));

/** How long a line stays when nobody touches it. */
const TOAST_MS = 7_000;

let frontChat: string | null = null;
let frontKing: string | null = null;
let made = 0;
let hide: ReturnType<typeof setTimeout> | undefined;

/** The chat on screen, so that what happens in it is not announced twice. Returns what clears it. */
export function chatInFront(chatId: string): () => void {
  frontChat = chatId;
  Background.dismiss(chatId);
  return () => {
    if (frontChat === chatId) frontChat = null;
  };
}

/** The computer whose King is on screen. Returns what clears it. */
export function kingInFront(hostId: string): () => void {
  frontKing = hostId;
  return () => {
    if (frontKing === hostId) frontKing = null;
  };
}

export function dismissToast(): void {
  clearTimeout(hide);
  useAlerts.setState({ toast: null });
}

export function openChat(chatId: string): void {
  dismissToast();
  if (frontChat === chatId) return;
  router.push({ pathname: '/chat/[chatId]', params: { chatId } });
}

/** Opens what the line is about. */
export function openToast(toast: Toast): void {
  if ('chatId' in toast) {
    openChat(toast.chatId);
    return;
  }
  dismissToast();
  if (frontKing === toast.kingOf) return;
  router.push({ pathname: '/host/[hostId]/king', params: { hostId: toast.kingOf } });
}

function buzz(kind: AlertKind): void {
  const type =
    kind === 'waiting'
      ? Haptics.NotificationFeedbackType.Warning
      : kind === 'failed'
        ? Haptics.NotificationFeedbackType.Error
        : Haptics.NotificationFeedbackType.Success;
  void Haptics.notificationAsync(type).catch(() => undefined);
}

/** A light tap: something the person did was taken. */
export function tap(): void {
  void Haptics.selectionAsync().catch(() => undefined);
}

const HEADLINE: Record<Exclude<AlertKind, 'king'>, (agent: string) => string> = {
  waiting: (agent) => `${agent} is waiting for you`,
  done: (agent) => `${agent} finished`,
  failed: (agent) => `${agent} stopped with an error`,
};

/** Says that the agent of a chat asked something, finished or failed. */
export function announce(
  chatId: string,
  kind: Exclude<AlertKind, 'king'>,
  agentName: string,
  line: string,
): void {
  const chat = useApp.getState().chats[chatId];
  if (!chat) return;
  const title = HEADLINE[kind](agentName);
  const body = [chat.title, line].filter(Boolean).join(' · ');
  if (AppState.currentState !== 'active') {
    // Read at a glance: paths as seen from the chat's folder.
    Background.alert(chatId, title, fromCwd(body, chat.cwd), `leon://chat/${encodeURIComponent(chatId)}`);
    return;
  }
  buzz(kind);
  if (frontChat === chatId) return;
  made += 1;
  useAlerts.setState({ toast: { key: made, chatId, kind, title, line: body } });
  clearTimeout(hide);
  hide = setTimeout(dismissToast, TOAST_MS);
}

/**
 * Says what King of a computer told the person. `echo`: it is in King's
 * conversation already, which needs no line while that is on screen.
 */
export function announceKing(hostId: string, title: string, body: string, echo: boolean): void {
  if (AppState.currentState !== 'active') return;
  if (echo && frontKing === hostId) return;
  buzz('king');
  made += 1;
  useAlerts.setState({ toast: { key: made, kingOf: hostId, kind: 'king', title: title || 'King', line: body } });
  clearTimeout(hide);
  hide = setTimeout(dismissToast, TOAST_MS);
}

/** Says why King would not do what the person asked: its own sentence. */
export function announceKingRefusal(hostId: string, sentence: string): void {
  buzz('failed');
  made += 1;
  useAlerts.setState({ toast: { key: made, kingOf: hostId, kind: 'failed', title: 'King refused', line: sentence } });
  clearTimeout(hide);
  hide = setTimeout(dismissToast, TOAST_MS);
}
