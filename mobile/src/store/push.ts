// King's notices while the app is closed. Off unless the person turns it on
// in the settings: then this phone asks for notifications, gets its Expo push
// token, and gives it to the King of each paired computer, which sends the
// title and body of its notices through Expo's push service while no client
// shows King. A computer that cannot be asked then is told the next time it
// connects. Turned off, each one is told to forget the token, also once it
// can be asked. Pressing such a notice opens that computer's King.
import Constants from 'expo-constants';
import * as Device from 'expo-device';
import * as Notifications from 'expo-notifications';
import { router } from 'expo-router';
import { Platform } from 'react-native';

import { kingHostFor, pushForgetRequest, pushRegisterRequest, pushWork, type PushIntent } from '@/king/push';

import { useApp } from './app';
import { ensureLink, useRuntime } from './runtime';

/** The Android channel King's notices arrive on. */
const CHANNEL = 'king';

/** What each computer was told since the app started, or since the setting changed. */
let told: Record<string, PushIntent> = {};
/** Computers being told right now. */
const telling = new Set<string>();

function projectId(): string | undefined {
  const extra = Constants.expoConfig?.extra as { eas?: { projectId?: string } } | undefined;
  return extra?.eas?.projectId ?? Constants.easConfig?.projectId;
}

/** Tells each computer that can be asked now what it has not been told yet. */
function tellHosts(): void {
  const app = useApp.getState();
  const runtime = useRuntime.getState();
  const work = pushWork({
    on: app.kingPush,
    token: app.pushToken,
    forgetting: app.pushForget,
    hosts: app.hosts.map((host) => ({
      hostId: host.hostId,
      online: runtime.conn[host.hostId]?.kind === 'online',
      king: runtime.shared[host.hostId]?.king === true,
    })),
    told,
  });
  for (const { hostId, intent } of work) {
    const token = app.pushToken;
    const opened = ensureLink(hostId);
    if (!token || !opened || telling.has(hostId)) continue;
    telling.add(hostId);
    const request =
      intent === 'register' ? pushRegisterRequest(token, Device.deviceName ?? 'phone') : pushForgetRequest(token);
    opened
      .kingAsk(request)
      .then(() => {
        told = { ...told, [hostId]: intent };
        if (intent === 'forget') {
          const left = useApp.getState().pushForget.filter((id) => id !== hostId);
          useApp.getState().setPush({ pushForget: left });
        }
      })
      // Refused or cut off: it is tried again the next time it connects.
      .catch(() => undefined)
      .finally(() => telling.delete(hostId));
  }
}

/**
 * Turns King's notices while closed on: asks for notifications and this
 * phone's push token, then gives it to every computer with a King. Returns
 * why not, in a sentence, when it could not be turned on.
 */
export async function enableKingPush(): Promise<string | undefined> {
  if (!Device.isDevice) return 'Push notifications need a real phone, not a simulator.';
  const id = projectId();
  if (!id) return 'This build of Leon has no push project set up.';
  try {
    if (Platform.OS === 'android') {
      await Notifications.setNotificationChannelAsync(CHANNEL, {
        name: 'King',
        importance: Notifications.AndroidImportance.DEFAULT,
      });
    }
    let { granted } = await Notifications.getPermissionsAsync();
    if (!granted) granted = (await Notifications.requestPermissionsAsync()).granted;
    if (!granted) return 'Notifications are not allowed for Leon. Allow them in the system settings.';
    const { data: token } = await Notifications.getExpoPushTokenAsync({ projectId: id });
    told = {};
    useApp.getState().setPush({ kingPush: true, pushToken: token, pushForget: [] });
  } catch (error) {
    return error instanceof Error && error.message ? error.message : 'No push token could be had.';
  }
  tellHosts();
  return undefined;
}

/** Turns them off: each paired computer is told to forget the token, now or once it connects. */
export function disableKingPush(): void {
  told = {};
  useApp.getState().setPush({
    kingPush: false,
    pushForget: useApp.getState().hosts.map((host) => host.hostId),
  });
  tellHosts();
}

/** Opens the King a pressed notice is about, if it is one of King's. */
function opened(response: Notifications.NotificationResponse | null | undefined): void {
  if (!response) return;
  const runtime = useRuntime.getState();
  const hostId = kingHostFor(
    response.notification.request.content.data,
    useApp.getState().hosts.map((host) => ({ hostId: host.hostId, king: runtime.shared[host.hostId]?.king === true })),
  );
  if (hostId) router.push({ pathname: '/host/[hostId]/king', params: { hostId } });
}

/**
 * Keeps the computers told while the app runs (each time one connects or
 * says it has a King), and opens King when one of its notices is pressed,
 * the one that started the app included. Returns what stops it.
 */
export function followKingPush(): () => void {
  // A notice that arrives while the app is open is shown all the same: the
  // service sends it only when no client shows King.
  Notifications.setNotificationHandler({
    handleNotification: async () => ({
      shouldShowBanner: true,
      shouldShowList: true,
      shouldPlaySound: false,
      shouldSetBadge: false,
    }),
  });
  const unsubscribe = useRuntime.subscribe((now, before) => {
    if (now.conn !== before.conn || now.shared !== before.shared) tellHosts();
  });
  tellHosts();
  const pressed = Notifications.addNotificationResponseReceivedListener(opened);
  void Notifications.getLastNotificationResponseAsync()
    .then((response) => {
      opened(response);
      if (response) void Notifications.clearLastNotificationResponseAsync();
    })
    .catch(() => undefined);
  return () => {
    unsubscribe();
    pressed.remove();
  };
}
