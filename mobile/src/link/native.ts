// The only file that imports the native module. It turns the generated
// bindings (tagged classes, bigint, thrown binding errors) into the plain
// values of ./types, so no screen and no store ever touches them.
import { Paths } from 'expo-file-system';
import {
  agents as nativeAgents,
  checkCode as nativeCheckCode,
  CodeCheck_Tags,
  ConnectionState_Tags,
  LeonMobile,
  LogoFormat,
  MobileError_Tags,
  Answer as NativeAnswer,
  OfflineReason as NativeOfflineReason,
  PermissionEffect,
  QuestionKind_Tags,
  TurnEvent_Tags,
  type CodeCheck as NativeCodeCheck,
  type ConnectionState as NativeConnectionState,
  type HistorySession as NativeHistorySession,
  type HostSessionLike,
  type QuestionKind as NativeQuestionKind,
  type TurnEvent as NativeTurnEvent,
  type TurnLike,
} from 'leon-link-native';

import {
  LinkError,
  type CodeCheck,
  type ConnState,
  type HistorySession,
  type HostLink,
  type LinkErrorCode,
  type LinkService,
  type OfflineReason,
  type PairedHost,
  type Answer,
  type ProjectLogo,
  type QuestionKind,
  type TurnEvent,
  type TurnHandle,
} from './types';

/** The folder the device's key lives in: the application's documents. */
function dataDir(): string {
  return decodeURI(Paths.document.uri).replace(/^file:\/\//, '').replace(/\/$/, '');
}

const ERROR_CODES = new Set<string>(Object.values(MobileError_Tags));

function toLinkError(error: unknown): LinkError {
  if (error instanceof LinkError) return error;
  const tagged = error as { tag?: unknown; inner?: { message?: unknown } } | null;
  if (tagged && typeof tagged.tag === 'string' && ERROR_CODES.has(tagged.tag)) {
    const detail = typeof tagged.inner?.message === 'string' ? tagged.inner.message : undefined;
    return new LinkError(tagged.tag as LinkErrorCode, detail);
  }
  return new LinkError('Internal', error instanceof Error ? error.message : String(error));
}

async function guard<T>(call: () => Promise<T>): Promise<T> {
  try {
    return await call();
  } catch (error) {
    throw toLinkError(error);
  }
}

function fromCodeCheck(check: NativeCodeCheck): CodeCheck {
  switch (check.tag) {
    case CodeCheck_Tags.Empty:
      return { kind: 'empty' };
    case CodeCheck_Tags.Incomplete:
      return { kind: 'incomplete', have: check.inner.have, need: check.inner.need };
    case CodeCheck_Tags.Complete:
      return { kind: 'complete', display: check.inner.display };
    case CodeCheck_Tags.BadCharacter:
      return { kind: 'bad-character', character: check.inner.character };
    case CodeCheck_Tags.TooLong:
      return { kind: 'too-long' };
  }
}

function fromReason(reason: NativeOfflineReason): OfflineReason {
  switch (reason) {
    case NativeOfflineReason.RelayUnreachable:
      return 'relay-unreachable';
    case NativeOfflineReason.HostOffline:
      return 'host-offline';
    case NativeOfflineReason.Revoked:
      return 'revoked';
    case NativeOfflineReason.VersionMismatch:
      return 'version-mismatch';
    case NativeOfflineReason.Other:
      return 'other';
  }
}

function fromState(state: NativeConnectionState): ConnState {
  switch (state.tag) {
    case ConnectionState_Tags.Connecting:
      return { kind: 'connecting' };
    case ConnectionState_Tags.Online:
      return { kind: 'online' };
    case ConnectionState_Tags.Offline:
      return {
        kind: 'offline',
        reason: fromReason(state.inner.reason),
        message: state.inner.message,
        attempt: state.inner.attempt,
      };
    case ConnectionState_Tags.Closed:
      return { kind: 'closed' };
  }
}

function fromQuestion(kind: NativeQuestionKind): QuestionKind {
  switch (kind.tag) {
    case QuestionKind_Tags.Permission:
      return {
        kind: 'permission',
        tool: kind.inner.tool,
        summary: kind.inner.summary,
        detail: kind.inner.detail,
        reason: kind.inner.reason,
        choices: kind.inner.choices.map((choice) => ({
          id: choice.id,
          effect:
            choice.effect === PermissionEffect.AllowOnce
              ? 'allow-once'
              : choice.effect === PermissionEffect.AllowAlways
                ? 'allow-always'
                : 'deny',
          label: choice.label,
          scope: choice.scope,
          takesReason: choice.takesReason,
        })),
      };
    case QuestionKind_Tags.Choices:
      return {
        kind: 'choices',
        questions: kind.inner.questions.map((q) => ({
          id: q.id,
          header: q.header,
          question: q.question,
          options: q.options.map((o) => ({ label: o.label, description: o.description })),
          multiSelect: q.multiSelect,
          allowOther: q.allowOther,
        })),
      };
    case QuestionKind_Tags.Plan:
      return { kind: 'plan', plan: kind.inner.plan };
  }
}

function toAnswer(answer: Answer): NativeAnswer {
  switch (answer.kind) {
    case 'permission':
      return new NativeAnswer.Permission({ choiceId: answer.choiceId, reason: answer.reason });
    case 'choices':
      return new NativeAnswer.Choices({
        answers: answer.answers.map((a) => ({
          questionId: a.questionId,
          labels: a.labels,
          other: a.other,
        })),
      });
    case 'plan':
      return new NativeAnswer.Plan({ approve: answer.approve, feedback: answer.feedback });
  }
}

function fromEvent(event: NativeTurnEvent): TurnEvent {
  switch (event.tag) {
    case TurnEvent_Tags.Started:
      return { type: 'started', sessionId: event.inner.sessionId, model: event.inner.model };
    case TurnEvent_Tags.TextDelta:
      return { type: 'text-delta', text: event.inner.text };
    case TurnEvent_Tags.Text:
      return { type: 'text', text: event.inner.text };
    case TurnEvent_Tags.Thought:
      return { type: 'thought', text: event.inner.text };
    case TurnEvent_Tags.ToolUse:
      return {
        type: 'tool-use',
        id: event.inner.id,
        name: event.inner.name,
        summary: event.inner.summary,
        detail: event.inner.detail,
      };
    case TurnEvent_Tags.ToolResult:
      return {
        type: 'tool-result',
        id: event.inner.id,
        ok: event.inner.ok,
        summary: event.inner.summary,
        detail: event.inner.detail,
      };
    case TurnEvent_Tags.Finished:
      return {
        type: 'finished',
        ok: event.inner.ok,
        error: event.inner.error,
        costUsd: event.inner.costUsd,
        durationMs:
          event.inner.durationMs === undefined ? undefined : Number(event.inner.durationMs),
        deniedTools: [...event.inner.deniedTools],
        interrupted: event.inner.interrupted,
      };
    case TurnEvent_Tags.Question:
      return { type: 'question', id: event.inner.id, ask: fromQuestion(event.inner.kind) };
    case TurnEvent_Tags.QuestionResolved:
      return { type: 'question-resolved', id: event.inner.id };
    case TurnEvent_Tags.ModeChanged:
      return { type: 'mode-changed', mode: event.inner.mode };
    case TurnEvent_Tags.Disconnected:
      return { type: 'disconnected' };
    case TurnEvent_Tags.Reconnected:
      return { type: 'reconnected' };
    case TurnEvent_Tags.Gap:
      return { type: 'gap' };
    case TurnEvent_Tags.Exited:
      return { type: 'exited', code: event.inner.code };
    case TurnEvent_Tags.Gone:
      return { type: 'gone' };
  }
}

function handleOf(turn: TurnLike): TurnHandle {
  // These write to the agent and may be refused: as a LinkError, like the rest.
  const tried = (call: () => void) => {
    try {
      call();
    } catch (error) {
      throw toLinkError(error);
    }
  };
  return {
    pty: turn.ptyId().toString(),
    cancel: () => turn.cancel(),
    answer: (questionId, answer) => tried(() => turn.answer(questionId, toAnswer(answer))),
    say: (text) => tried(() => turn.send(text)),
    interrupt: () => tried(() => turn.interrupt()),
    setMode: (mode) => tried(() => turn.setMode(mode)),
    setModel: (model) => tried(() => turn.setModel(model)),
    unsent: () => turn.unsent(),
  };
}

function historySession(s: NativeHistorySession): HistorySession {
  return {
    agent: s.agent,
    externalId: s.externalId,
    cwd: s.cwd,
    title: s.title,
    model: s.model,
    startedMs: Number(s.startedMs),
    updatedMs: Number(s.updatedMs),
    messages: s.messages,
    canResume: s.canResume,
  };
}

function wrapSession(session: HostSessionLike): HostLink {
  return {
    state: () => fromState(session.state()),
    hostName: () => session.hostName(),
    reconnectNow: () => session.reconnectNow(),
    close: () => session.close(),
    shareState: () =>
      guard(async () => {
        const shared = await session.shareState();
        return {
          projects: shared.projects.map((p) => ({ name: p.name, root: p.root })),
          sessions: shared.sessions.map(historySession),
          truncated: shared.truncated,
          // An installed build older than the pins has no such field.
          pinned: (shared.pinned ?? []).map(historySession),
          den: shared.den === true,
          king: shared.king === true,
        };
      }),
    den: () =>
      guard(async () =>
        (await session.den()).map((cub) => ({
          id: cub.id.toString(),
          name: cub.name,
          agent: cub.agent,
          state: cub.state,
          detail: cub.detail,
          level: cub.level,
          parent: cub.parent?.toString(),
          mystery: cub.mystery,
          externalId: cub.externalId,
          cwd: cub.cwd,
        })),
      ),
    transcript: (agent, externalId) =>
      guard(async () => {
        const entries = await session.transcript(agent, externalId);
        return entries?.map((e) => ({ role: e.role, text: e.text, atMs: Number(e.atMs) }));
      }),
    homeDir: () => guard(() => session.homeDir()),
    subdirs: (path) => guard(() => session.subdirs(path)),
    worktrees: (roots) =>
      guard(async () =>
        (await session.worktrees(roots)).map((trees) =>
          trees.map((tree) => ({ path: tree.path, branch: tree.branch, main: tree.main })),
        ),
      ),
    projectLogo: (root) =>
      guard(async (): Promise<ProjectLogo> => {
        const { image, avatarUrl } = await session.projectLogo(root);
        if (image?.format === LogoFormat.Svg) return { kind: 'svg', xml: image.data, hash: image.hash };
        if (image) {
          const format =
            image.format === LogoFormat.Jpeg ? 'jpeg' : image.format === LogoFormat.Webp ? 'webp' : 'png';
          return { kind: 'image', format, base64: image.data, hash: image.hash };
        }
        return avatarUrl ? { kind: 'avatar', url: avatarUrl } : { kind: 'none' };
      }),
    installedAgents: () => guard(() => session.installedAgents()),
    agentOptions: (agent) =>
      guard(async () => {
        const options = await session.agentOptions(agent);
        return {
          version: options.version,
          modes: options.modes.map((m) => ({
            id: m.id,
            label: m.label,
            description: m.description,
            dangerous: m.dangerous,
          })),
          defaultMode: options.defaultMode,
          models: options.models.map((m) => ({
            id: m.id,
            label: m.label,
            description: m.description,
            efforts: [...m.efforts],
          })),
          questions: options.questions,
          followUps: options.followUps,
          liveChanges: options.liveChanges,
          assumed: options.assumed,
        };
      }),
    startTurn: (request, onEvent) =>
      guard(async () =>
        handleOf(
          await session.startTurn(
            {
              agent: request.agent,
              cwd: request.cwd,
              prompt: request.prompt,
              resumeId: request.resumeId,
              mode: request.options.mode,
              model: request.options.model,
              effort: request.options.effort,
              addDirs: [],
              chatId: request.chatId,
            },
            { onEvent: (event) => onEvent(fromEvent(event)) },
          ),
        ),
      ),
    runningTurns: () =>
      guard(async () =>
        (await session.runningTurns()).map((t) => ({
          pty: t.pty.toString(),
          agent: t.agent,
          chatId: t.chatId,
          cwd: t.cwd,
          startedUnix: Number(t.startedUnix),
          exitCode: t.exitCode,
        })),
      ),
    attachTurn: (pty, onEvent) =>
      guard(async () =>
        handleOf(
          await session.attachTurn(BigInt(pty), {
            onEvent: (event) => onEvent(fromEvent(event)),
          }),
        ),
      ),
    kingAsk: (request) =>
      guard(async (): Promise<unknown> => {
        // A reply without a result (`say`) comes as `null`.
        const result = await session.kingAsk(JSON.stringify(request));
        return result ? JSON.parse(result) : null;
      }),
    kingWatch: (onNotice, onClosed) => {
      try {
        const watch = session.kingWatch({
          onNotice: (notice) => {
            let read: unknown;
            try {
              read = JSON.parse(notice);
            } catch {
              // Not JSON: nothing this app could read in it.
              return;
            }
            onNotice(read);
          },
          onClosed,
        });
        return { stop: () => watch.stop() };
      } catch (error) {
        throw toLinkError(error);
      }
    },
  };
}

/** The service backed by the Rust library. */
export function createNativeLink(initialDeviceName: string): LinkService {
  let name = initialDeviceName;
  let device: LeonMobile | null = null;

  // The identity is read (or made, the first time) when it is first needed.
  const mobile = (): LeonMobile => {
    if (!device) {
      try {
        device = new LeonMobile(dataDir(), name);
      } catch (error) {
        throw toLinkError(error);
      }
    }
    return device;
  };

  return {
    deviceId: () => mobile().deviceId(),
    deviceName: () => name,
    setDeviceName: (next) => {
      if (next === name) return;
      name = next;
      device = null;
    },
    agents: () =>
      nativeAgents().map((a) => ({
        id: a.id,
        name: a.name,
        program: a.program,
      })),
    checkCode: (input) => fromCodeCheck(nativeCheckCode(input)),
    pair: (relayUrl, code) =>
      guard(async () => {
        const host = await mobile().pair(relayUrl, code);
        const paired: PairedHost = {
          hostId: host.hostId,
          hostKeyHex: host.hostKeyHex,
          hostName: host.hostName,
          relayUrl: host.relayUrl,
        };
        return paired;
      }),
    connect: (host, onState) => {
      try {
        return wrapSession(
          mobile().connect(host, { onState: (state) => onState(fromState(state)) }),
        );
      } catch (error) {
        throw toLinkError(error);
      }
    },
  };
}
