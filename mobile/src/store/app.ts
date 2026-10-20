// What the application remembers: its settings, the computers it paired with
// and the chats it knows. Messages are stored apart, one key per chat
// (./runtime.ts), so that a long chat is not rewritten with every change here.
import * as Device from 'expo-device';
import { create } from 'zustand';
import { persist } from 'zustand/middleware';

import type { ChatRecord } from '@/chat/list';
import type { PairedHost, TurnOptions } from '@/link/types';

import { persistStorage } from './storage';

export const DEFAULT_RELAY_URL = 'wss://relay.getleon.dev';

/** Which Leon theme to wear: dark unless the person says otherwise. */
export type Appearance = 'dark' | 'light' | 'system';

/** How lists of sessions are ordered: as the computer groups them, or the newest first. */
export type SessionOrder = 'grouped' | 'recent';

export type HostRecord = PairedHost & { pairedAt: number };

type AppState = {
  deviceName: string;
  relayUrl: string;
  appearance: Appearance;
  sessionOrder: SessionOrder;
  hosts: HostRecord[];
  chats: Record<string, ChatRecord>;
  /** The options last used with each agent of each computer: what a new chat starts with. */
  lastOptions: Record<string, TurnOptions>;
  /** King's notices while the app is closed, through Expo's push service: off unless turned on. */
  kingPush: boolean;
  /** This phone's Expo push token, once it was asked for one. */
  pushToken?: string;
  /** The computers still to be told to forget the token, since the setting was turned off. */
  pushForget: string[];
  setPush(change: Partial<Pick<AppState, 'kingPush' | 'pushToken' | 'pushForget'>>): void;
  rememberOptions(hostId: string, agent: string, options: TurnOptions): void;
  setDeviceName(name: string): void;
  setRelayUrl(url: string): void;
  setAppearance(appearance: Appearance): void;
  setSessionOrder(order: SessionOrder): void;
  addHost(host: PairedHost): void;
  removeHost(hostId: string): string[];
  putChat(chat: ChatRecord): void;
  patchChat(id: string, change: Partial<ChatRecord>): void;
  removeChat(id: string): void;
};

/** The key of one agent on one computer. */
export function optionsKey(hostId: string, agent: string): string {
  return `${hostId}/${agent}`;
}

function defaultDeviceName(): string {
  return (Device.deviceName ?? Device.modelName ?? 'Phone').slice(0, 64);
}

export const useApp = create<AppState>()(
  persist(
    (set, get) => ({
      deviceName: defaultDeviceName(),
      relayUrl: DEFAULT_RELAY_URL,
      appearance: 'dark',
      sessionOrder: 'grouped',
      hosts: [],
      chats: {},
      lastOptions: {},
      kingPush: false,
      pushForget: [],
      setPush: (change) => set(change),
      rememberOptions: (hostId, agent, options) =>
        set((state) => ({
          lastOptions: { ...state.lastOptions, [optionsKey(hostId, agent)]: options },
        })),
      setDeviceName: (name) => set({ deviceName: name.trim().slice(0, 64) || defaultDeviceName() }),
      setRelayUrl: (url) => set({ relayUrl: url.trim() || DEFAULT_RELAY_URL }),
      setAppearance: (appearance) => set({ appearance }),
      setSessionOrder: (sessionOrder) => set({ sessionOrder }),
      addHost: (host) =>
        set((state) => ({
          hosts: [
            ...state.hosts.filter((h) => h.hostId !== host.hostId),
            { ...host, pairedAt: Date.now() },
          ],
        })),
      removeHost: (hostId) => {
        const gone = Object.values(get().chats)
          .filter((c) => c.hostId === hostId)
          .map((c) => c.id);
        set((state) => ({
          hosts: state.hosts.filter((h) => h.hostId !== hostId),
          chats: Object.fromEntries(
            Object.entries(state.chats).filter(([, c]) => c.hostId !== hostId),
          ),
        }));
        return gone;
      },
      putChat: (chat) => set((state) => ({ chats: { ...state.chats, [chat.id]: chat } })),
      patchChat: (id, change) =>
        set((state) => {
          const chat = state.chats[id];
          return chat ? { chats: { ...state.chats, [id]: { ...chat, ...change } } } : state;
        }),
      removeChat: (id) =>
        set((state) => {
          const { [id]: _gone, ...rest } = state.chats;
          return { chats: rest };
        }),
    }),
    {
      name: 'leon.app.v1',
      storage: persistStorage,
      partialize: ({
        deviceName,
        relayUrl,
        appearance,
        sessionOrder,
        hosts,
        chats,
        lastOptions,
        kingPush,
        pushToken,
        pushForget,
      }) => ({
        deviceName,
        relayUrl,
        appearance,
        sessionOrder,
        hosts,
        chats,
        lastOptions,
        kingPush,
        pushToken,
        pushForget,
      }),
    },
  ),
);
