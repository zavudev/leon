// King's notices on a phone whose application is closed: which computers are
// told this phone's push token, or told to forget it, and which computer a
// notice that was pressed is about. Pure; the store (`@/store/push`) does it.
import type { KingRequest } from '@/link/types';

/** What a computer is told: to send King's notices here, or to stop. */
export type PushIntent = 'register' | 'forget';

/** A paired computer, as far as this matters: whether it can be asked now, and has a King. */
export type PushHost = { hostId: string; online: boolean; king: boolean };

export function pushRegisterRequest(token: string, name: string): KingRequest {
  return { op: 'push_register', token, name: name.trim() || 'phone' };
}

export function pushForgetRequest(token: string): KingRequest {
  return { op: 'push_forget', token };
}

/**
 * What to tell which computer now. On, every computer with a King that can
 * be asked is told the token, once (`told` is what was told since the app
 * started, or since the setting changed). Off, the computers still to forget
 * it (`forgetting`) are told so once they can be asked. Without a token,
 * nothing.
 */
export function pushWork(input: {
  on: boolean;
  token?: string;
  forgetting: string[];
  hosts: PushHost[];
  told: Record<string, PushIntent>;
}): { hostId: string; intent: PushIntent }[] {
  const { on, token, forgetting, hosts, told } = input;
  if (!token) return [];
  const intent: PushIntent = on ? 'register' : 'forget';
  return hosts
    .filter((host) => host.online && host.king && told[host.hostId] !== intent)
    .filter((host) => on || forgetting.includes(host.hostId))
    .map((host) => ({ hostId: host.hostId, intent }));
}

/**
 * The computer whose King a pressed notice opens: the one it names (`host`,
 * should the service ever say), else the first known to have a King, else
 * the first paired one. Nothing when the notice is not King's.
 */
export function kingHostFor(
  data: unknown,
  hosts: { hostId: string; king: boolean }[],
): string | undefined {
  const json = typeof data === 'object' && data !== null ? (data as Record<string, unknown>) : undefined;
  if (json?.king !== true) return undefined;
  const named = typeof json.host === 'string' ? hosts.find((h) => h.hostId === json.host) : undefined;
  return (named ?? hosts.find((h) => h.king) ?? hosts[0])?.hostId;
}
