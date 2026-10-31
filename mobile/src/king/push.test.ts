import { describe, expect, test } from 'bun:test';

import { kingHostFor, pushForgetRequest, pushRegisterRequest, pushWork, type PushHost } from './push';

const TOKEN = 'ExponentPushToken[abc]';
const HOSTS: PushHost[] = [
  { hostId: 'a', online: true, king: true },
  { hostId: 'b', online: false, king: true },
  { hostId: 'c', online: true, king: false },
  { hostId: 'd', online: true, king: true },
];

describe("Orchestrator's notices while closed", () => {
  test('the requests', () => {
    expect(pushRegisterRequest(TOKEN, ' Pixel 9 ')).toEqual({ op: 'push_register', token: TOKEN, name: 'Pixel 9' });
    expect(pushRegisterRequest(TOKEN, '  ')).toEqual({ op: 'push_register', token: TOKEN, name: 'phone' });
    expect(pushForgetRequest(TOKEN)).toEqual({ op: 'push_forget', token: TOKEN });
  });

  test('on, each computer with an Orchestrator that can be asked is told the token once', () => {
    const work = pushWork({ on: true, token: TOKEN, forgetting: [], hosts: HOSTS, told: {} });
    expect(work).toEqual([
      { hostId: 'a', intent: 'register' },
      { hostId: 'd', intent: 'register' },
    ]);
    expect(pushWork({ on: true, token: TOKEN, forgetting: [], hosts: HOSTS, told: { a: 'register' } })).toEqual([
      { hostId: 'd', intent: 'register' },
    ]);
    // Back online, the one that was not is told.
    const online = HOSTS.map((h) => ({ ...h, online: true }));
    expect(
      pushWork({ on: true, token: TOKEN, forgetting: [], hosts: online, told: { a: 'register', d: 'register' } }),
    ).toEqual([{ hostId: 'b', intent: 'register' }]);
  });

  test('off, only the computers still to forget it are told, once they can be asked', () => {
    expect(pushWork({ on: false, token: TOKEN, forgetting: ['b', 'd'], hosts: HOSTS, told: {} })).toEqual([
      { hostId: 'd', intent: 'forget' },
    ]);
    expect(pushWork({ on: false, token: TOKEN, forgetting: ['d'], hosts: HOSTS, told: { d: 'forget' } })).toEqual([]);
    // One that registered before is told to forget, all the same.
    expect(pushWork({ on: false, token: TOKEN, forgetting: ['a'], hosts: HOSTS, told: { a: 'register' } })).toEqual([
      { hostId: 'a', intent: 'forget' },
    ]);
  });

  test('without a token nothing is told', () => {
    expect(pushWork({ on: true, forgetting: ['a'], hosts: HOSTS, told: {} })).toEqual([]);
  });

  test("a pressed notice opens the Orchestrator it names, else the first computer with one", () => {
    const hosts = [
      { hostId: 'x', king: false },
      { hostId: 'y', king: true },
      { hostId: 'z', king: true },
    ];
    expect(kingHostFor({ king: true }, hosts)).toBe('y');
    expect(kingHostFor({ king: true, host: 'z' }, hosts)).toBe('z');
    expect(kingHostFor({ king: true, host: 'gone' }, hosts)).toBe('y');
    expect(kingHostFor({ king: true }, [{ hostId: 'x', king: false }])).toBe('x');
    expect(kingHostFor({ king: true }, [])).toBeUndefined();
    expect(kingHostFor({ chat: 'c1' }, hosts)).toBeUndefined();
    expect(kingHostFor(null, hosts)).toBeUndefined();
  });
});
