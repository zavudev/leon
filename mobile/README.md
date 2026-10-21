# Leon for phones

A chat for iPhone and Android that pairs with a computer running Leon, sends a
prompt to a coding agent there (Claude Code, Codex or opencode) and shows the answer as
it is written. The agent runs on the computer, in a terminal the host owns, so
a turn keeps going when the phone locks or loses its connection, and the app
follows it again when it comes back.

Nothing of the protocol is written in TypeScript: pairing, the encrypted
session and the turns are the Rust crate [`crates/leon-mobile`](../crates/leon-mobile),
exposed to React Native by [`modules/leon-link-native`](modules/leon-link-native).

> **Status.** Typechecked, linted, unit-tested and bundled for both platforms.
> It has **not** been built or run on a device or simulator yet: the machine it
> was written on has no Android SDK and no Xcode.

## What is here

| Path | What |
| --- | --- |
| `src/app` | the screens (Expo Router): computers, pairing (type, paste or scan the computer's QR; `leon://pair?code=…&relay=…` opens it filled in), chats of a computer, new chat, chat, King of a computer and its workers, settings |
| `src/link` | the only code that touches the native module: `types.ts` (plain values), `native.ts` (the bindings behind them), `errors.ts` (what to tell the person) |
| `src/chat` | pure logic with tests: turn events to chat rows, the chat list, merging a shared transcript, the Den's lions |
| `src/king` | pure logic with tests: King's status, notices and conversations read from its JSON, merged by number |
| `src/store` | `app.ts` (settings, paired computers, chats; persisted), `runtime.ts` (connections, turns in flight, messages), `king.ts` (King of each computer while its screen is open), `alerts.ts` and `background.ts` (telling the person, in front and behind) |
| `src/theme`, `src/ui` | the Leon theme from [`brand/tokens.md`](../brand/tokens.md) and the few components built on it |
| `modules/leon-link-native` | the generated Turbo Module around the Rust crate |
| `modules/leon-background` | Android only: the foreground service that keeps the link behind other apps, and the notifications |
| `assets` | icon and splash, rendered from `brand/logo/final`; `assets/den`, the Den's lions in 2.5D, drawn by the desktop's renderer (`cargo run -p leon-den --features den3d --example den_lions_png -- mobile/assets/den`) |

Built with Expo SDK 57 (React Native 0.86, New Architecture), Expo Router,
Zustand, FlashList, react-native-keyboard-controller and
react-native-markdown-display. State is persisted with `expo-sqlite/kv-store`:
synchronous, so the saved state is there before the first frame, and Expo's own
module rather than one more native dependency.

## Prerequisites

* [Bun](https://bun.sh) 1.4 and Node 22 or newer.
* Rust (the repository's `rust-toolchain.toml` picks the version).
* **Android:** Android Studio or the command-line SDK, the NDK
  (`ANDROID_NDK_HOME`), a JDK 17, `cargo install cargo-ndk`, and
  `rustup target add aarch64-linux-android x86_64-linux-android`.
* **iOS:** a Mac with Xcode and CocoaPods, and
  `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`.

The app cannot run in Expo Go: it has its own native module, so it needs a
development build.

## Run it against your own computer

The public relay (`wss://relay.getleon.dev`) may not be live yet. A local one
is enough to try everything, with the phone and the computer on the same
network:

```sh
# 1. A relay, on the computer (from the repository root)
cargo run -p leon-mobile --example dev-relay -- 0.0.0.0:8787

# 2. A host that prints a pairing code
LEON_RELAY_URL=ws://127.0.0.1:8787 leon host --pair
```

The headless `leon host` shares no history, so the app lists only the chats
you start from the phone. To see the computer's projects and past sessions,
run the Leon app instead, set its relay to `ws://127.0.0.1:8787` (Settings,
Machines, Relay server) and switch on **Share this machine**.

```sh
# 3. The app (from mobile/)
bun install
bun run native:android      # the Rust library for Android; or native:ios on a Mac
bun run android             # builds and installs the development build; or: bun run ios
bun run start               # later runs: only the JavaScript bundler
```

In the app choose **Connect a computer**, set **Relay server** to
`ws://<the computer's LAN address>:8787` and type the code. The development
relay has no TLS and no limits: use it on a network you trust only.

Run `bun run native:android` (or `native:ios`) again whenever the Rust crate
changes; when its exported API changes, `bun run native:generate` first
regenerates the bindings (that step needs only Rust).

## Builds with EAS

`eas.json` has four profiles: `development` (a development build for devices),
`development-simulator`, `preview` (an installable internal build) and
`production`. The Rust libraries are not committed, so two hooks
(`scripts/eas-build-native.sh`) install Rust on the builder and compile them
for the platform being built: after the dependencies on Android, and before
them on iOS, where `pod install` already needs the framework.

```sh
bunx eas-cli login
bunx eas-cli init                                 # links the project to your Expo account
bunx eas-cli build --profile development --platform android
bunx eas-cli build --profile development --platform ios
```

`ios.bundleIdentifier` and `android.package` are both `dev.zavu.leon`: Leon is
Zavu's, whose domain is zavu.dev. An iOS build for devices needs an Apple Developer account that owns
that identifier.

## Checks

```sh
bun run check     # typecheck (app and module), lint, unit tests
bun run doctor    # expo-doctor
```

The unit tests cover the logic that has no screen: the reducer from turn
events to chat rows (streaming, a whole message replacing its draft, tools,
refused tools, a finish the library made up, a replayed turn, a gap), the chat
list, the transcript merge and what King's service answers. They run with
`bun test` and never load the native module, which screens reach only through
`src/link`.

## Security

* A paired phone can run any command on the computer as you. That is the
  feature: pair only your own devices, and revoke a lost one on the computer
  (Share this machine, or `leon host revoke`).
* Everything between the phone and the computer is end-to-end encrypted
  (SPAKE2 pairing, then Noise); the relay forwards ciphertext it cannot read.
* The threat model, the protocol and the limits are in
  [docs/REMOTE.md](../docs/REMOTE.md). This code has had no independent
  security review.

The mode under the composer is the agent's own permission mode, with its
model and effort: the list is what the agent installed on that computer says
it offers. What the mode does not let it do by itself, the agent asks, and the
question is answered on the phone: a tool it wants to use (once, always, or
no, with a reason), a question with options, a plan to approve. An answer
given while offline is kept and sent when the connection is back. A mode that
lets the agent do anything asks you to confirm.

The agents are driven this way:

* **Claude Code** through its control protocol. It can be told more while it
  works, and its mode and model changed.
* **Codex** through its application server. Its modes are what it asks
  before doing and what its sandbox allows, fixed for a turn once it started.
  Its refusals take no reason, so what to do instead is your next message.
* **opencode** through the Agent Client Protocol (`opencode acp`). Its modes
  are its own agents (Build, Plan, and any you defined), plus one of this
  app's: Build with every permission allowed at once, as `opencode --auto`
  does. What it asks depends on the `permission` settings of opencode on that
  computer: by default it asks little. Refusing a permission ends its turn,
  and it cannot be told more while it works. Its models are every provider's,
  so the list is searched; the efforts offered are the usual ones, and one a
  model lacks is left at that computer's default.
* **Grok** through the same protocol (`grok agent stdio`, grok 1.0.13 or
  later: an older one is refused by its own service). It has no modes of its
  own to set, so the two offered are this app's: asking before changes, and
  never asking. Refusing a permission ends its turn.
* **Cursor** through the same protocol (`cursor-agent acp`), after
  `cursor-agent login` on that computer. **Not checked against a real turn
  yet**; no model is offered, and the conversation runs with its own.

No other agent can be driven in this version; their sessions are shown and
cannot be continued.

## Known limits

* **Alerts behind another app are Android only, and only while a turn runs.**
  A foreground service (`modules/leon-background`, type `dataSync`) keeps the
  link while a turn sent from this phone is running, so a question or the end
  of the turn arrives as a system notification, also with the screen off. It
  stops when no turn runs, and when the app is swiped away. Android 15 allows
  this kind of service six hours a day behind other apps. On iOS nothing
  arrives until the app is opened again: that needs the relay to push. A turn
  that finished meanwhile is fetched when you come back, for as long as the
  computer keeps it (half an hour after it ended, the last 2 MiB).
* **The device's key is a file in the app's sandbox**, not in the Keychain or
  the Keystore. It is protected by the system's app isolation and the device's
  encryption, and is included in device backups.
* **History comes only from a desktop Leon.** A headless `leon host` shares
  nothing, and a turn made from the phone appears in the computer's history
  only once its Leon imports it.
* **Five agents.** Claude Code, Codex, opencode, Grok and Cursor; sessions of
  other agents can be read but not continued.
* **Unix hosts.** A turn starts through the owner's login shell, and browsing
  folders assumes `/` paths.
* `wss://` uses the Mozilla root certificates bundled with the app
  (`webpki-roots`), not the system's trust store.

## Android toolchain without root

`scripts/android-env.sh` puts a user-level toolchain on `PATH`: JDK 17 from
mise (`mise install java@temurin-17`) and an SDK in `~/Android/Sdk` holding
`platform-tools`, `platforms;android-36`, `build-tools;36.0.0`,
`ndk;27.1.12297006`, `cmake;3.22.1` and, for the emulator, `emulator` plus
`system-images;android-36;google_apis;x86_64`. Source it, then run
`bun run native:android` and `bun run android`. From the emulator the computer
is `10.0.2.2`, so a relay started there is `ws://10.0.2.2:8787`.
