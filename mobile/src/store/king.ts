// King of each paired computer, as far as this phone follows it: its status,
// its conversation and those of the workers being looked at. Only while one
// of its screens is open: a watch on King's notices says when anything
// changed, and only what came after the newest entry known is asked for.
//
// A computer is asked only when its shared state says it has a King: one that
// does not know the request drops the connection.
import { create } from 'zustand';

import {
  lastSeq,
  mergeEntries,
  parseEntries,
  parseNotice,
  parseStatus,
  TRANSCRIPT_LIMIT,
  type KingEntry,
  type KingStatus,
} from '@/king/model';
import { explain } from '@/link/errors';
import { LinkError, type KingTarget, type KingWatch } from '@/link/types';

import { announceKing } from './alerts';
import { ensureLink, refreshShared, useRuntime } from './runtime';

type KingState = {
  /** King's status per computer, as it last said. */
  status: Record<string, KingStatus>;
  /** King's conversation per computer: the number of that conversation, and its entries. */
  brain: Record<string, { conversation: number; entries: KingEntry[] }>;
  /** The conversations of the workers being looked at, by `workerKey`. */
  workers: Record<string, KingEntry[]>;
  /** Why King could not be asked, per computer, until it answers again. */
  error: Record<string, string>;
};

export const useKing = create<KingState>(() => ({ status: {}, brain: {}, workers: {}, error: {} }));

export function workerKey(hostId: string, worker: number): string {
  return `${hostId}/${worker}`;
}

function canAsk(hostId: string): boolean {
  const runtime = useRuntime.getState();
  return runtime.conn[hostId]?.kind === 'online' && runtime.shared[hostId]?.king === true;
}

/** What went wrong in a sentence: King's own, when it refused. */
export function kingRefusal(error: unknown): string {
  if (error instanceof LinkError && error.code === 'Host' && error.detail) return error.detail;
  return explain(error).headline;
}

function setError(hostId: string, error: unknown): void {
  const said = error === undefined ? undefined : kingRefusal(error);
  useKing.setState((s) => {
    const next = { ...s.error };
    if (said) next[hostId] = said;
    else delete next[hostId];
    return { error: next };
  });
}

// One reading of each conversation at a time: one asked for meanwhile is
// made once the first is over, with what it brought.
const reading = new Map<string, Promise<void>>();
const again = new Set<string>();

function once(key: string, read: () => Promise<void>): Promise<void> {
  const going = reading.get(key);
  if (going) {
    again.add(key);
    return going;
  }
  const run = async (): Promise<void> => {
    try {
      await read();
    } finally {
      if (again.delete(key)) await run();
    }
  };
  const started = run().finally(() => reading.delete(key));
  reading.set(key, started);
  return started;
}

/** Reads what a conversation said after the newest entry known. */
function readTranscript(hostId: string, target: KingTarget): Promise<void> {
  const key = target === 'brain' ? `${hostId}/brain` : workerKey(hostId, target.worker);
  return once(key, async () => {
    const opened = ensureLink(hostId);
    if (!opened || !canAsk(hostId)) return;
    const known = (): KingEntry[] =>
      target === 'brain' ? (useKing.getState().brain[hostId]?.entries ?? []) : (useKing.getState().workers[key] ?? []);
    const conversation = useKing.getState().brain[hostId]?.conversation ?? 0;
    const since = lastSeq(known());
    let read: KingEntry[];
    try {
      read = parseEntries(
        await opened.kingAsk({ op: 'transcript', target, since, limit: TRANSCRIPT_LIMIT }),
      );
    } catch (error) {
      setError(hostId, error);
      return;
    }
    useKing.setState((s) => {
      if (target !== 'brain') return { workers: { ...s.workers, [key]: mergeEntries(known(), read) } };
      // King moved to another conversation while this was asked: it is read again.
      if ((s.brain[hostId]?.conversation ?? 0) !== conversation) return {};
      return { brain: { ...s.brain, [hostId]: { conversation, entries: mergeEntries(known(), read) } } };
    });
  });
}

/** The workers whose conversation is on screen, per computer, with how many screens show each. */
const shownWorkers = new Map<string, number>();

/** Keeps a status, and reads what is new in the conversations it says moved. */
async function applyStatus(hostId: string, status: KingStatus): Promise<void> {
  useKing.setState((s) => {
    const brain = s.brain[hostId];
    // Another conversation starts its entries again from nothing.
    const fresh = !brain || brain.conversation !== status.conversation;
    return {
      status: { ...s.status, [hostId]: status },
      brain: fresh ? { ...s.brain, [hostId]: { conversation: status.conversation, entries: [] } } : s.brain,
    };
  });
  setError(hostId, undefined);
  const reads: Promise<void>[] = [];
  const brain = useKing.getState().brain[hostId]?.entries ?? [];
  if (brain.length === 0 || (status.brain.lastSeq ?? -1) > (lastSeq(brain) ?? -1)) {
    reads.push(readTranscript(hostId, 'brain'));
  }
  for (const worker of status.workers) {
    const key = workerKey(hostId, worker.id);
    if (!shownWorkers.has(key)) continue;
    const entries = useKing.getState().workers[key] ?? [];
    if (entries.length === 0 || (worker.conversation.lastSeq ?? -1) > (lastSeq(entries) ?? -1)) {
      reads.push(readTranscript(hostId, { worker: worker.id }));
    }
  }
  await Promise.all(reads);
}

/** Asks King for its status again, and reads what is new. */
export function refreshKing(hostId: string): Promise<void> {
  return once(`${hostId}/status`, async () => {
    const opened = ensureLink(hostId);
    if (!opened) return;
    if (!useRuntime.getState().shared[hostId]) await refreshShared(hostId, false);
    if (!canAsk(hostId)) return;
    let status: KingStatus | undefined;
    try {
      status = parseStatus(await opened.kingAsk({ op: 'status' }));
    } catch (error) {
      setError(hostId, error);
      return;
    }
    if (status) await applyStatus(hostId, status);
  });
}

/** Says something to King. Throws what refused it, for the screen to say. */
export async function sayToKing(hostId: string, text: string): Promise<void> {
  const opened = ensureLink(hostId);
  if (!opened || !canAsk(hostId)) throw new LinkError('Offline');
  await opened.kingAsk({ op: 'say', text });
  await refreshKing(hostId);
}

const watchers = new Map<string, { count: number; stop(): void }>();

/**
 * Follows King of a computer while one of its screens is open: reads it now,
 * then again on each of its notices and each time the connection comes back.
 * What King tells the person is said across the top. Returns what stops it.
 */
export function watchKing(hostId: string): () => void {
  const known = watchers.get(hostId);
  if (known) {
    known.count += 1;
  } else {
    let watch: KingWatch | undefined;
    let stopped = false;
    const open = () => {
      const opened = ensureLink(hostId);
      if (stopped || watch || !opened || !canAsk(hostId)) return;
      try {
        watch = opened.kingWatch(
          (notice) => {
            const read = parseNotice(notice);
            if (read?.kind === 'changed') void applyStatus(hostId, read.status);
            else if (read?.kind === 'notify') announceKing(hostId, read.title, read.body, read.echo);
          },
          // What was said meanwhile is not known: it is asked for.
          () => void refreshKing(hostId),
        );
      } catch (error) {
        setError(hostId, error);
      }
    };
    // Online again, or told it has a King: read it, and listen.
    const unsubscribe = useRuntime.subscribe((now, before) => {
      const online = now.conn[hostId]?.kind === 'online';
      const wasOnline = before.conn[hostId]?.kind === 'online';
      const king = now.shared[hostId]?.king === true;
      const hadKing = before.shared[hostId]?.king === true;
      if ((online && !wasOnline) || (king && !hadKing)) {
        open();
        void refreshKing(hostId);
      }
    });
    void refreshKing(hostId).then(open);
    watchers.set(hostId, {
      count: 1,
      stop: () => {
        stopped = true;
        unsubscribe();
        watch?.stop();
      },
    });
  }
  return () => {
    const held = watchers.get(hostId);
    if (!held) return;
    held.count -= 1;
    if (held.count > 0) return;
    held.stop();
    watchers.delete(hostId);
  };
}

/**
 * Follows the conversation of one worker while its screen is open, on top of
 * `watchKing`: it is read now, and again whenever King's status says it moved.
 */
export function watchWorker(hostId: string, worker: number): () => void {
  const key = workerKey(hostId, worker);
  shownWorkers.set(key, (shownWorkers.get(key) ?? 0) + 1);
  const stopKing = watchKing(hostId);
  void readTranscript(hostId, { worker });
  return () => {
    stopKing();
    const left = (shownWorkers.get(key) ?? 1) - 1;
    if (left > 0) shownWorkers.set(key, left);
    else shownWorkers.delete(key);
  };
}
