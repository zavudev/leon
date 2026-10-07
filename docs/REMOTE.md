# Remote machines through a relay

"Connect a machine, with a code" lets Leon drive another computer without SSH
and without opening any port: you install Leon there, switch on **Share this
machine**, and it shows a short code. Both computers dial **out** to a relay,
which pairs them and forwards bytes. Everything between the two Leon
installations is end-to-end encrypted, so the relay cannot read or alter it.

The relay service is operated by Zavu and its server is not part of this
repository. This document specifies what a relay must do and what it can and
cannot see; the security-relevant code (`leon-wire`, `leon-link`, `leon-host`)
is all here so it can be reviewed without trusting any server.

> **Status.** The relay at `wss://relay.getleon.dev` is not deployed yet. Leon
> detects an unreachable relay and says so, naming the address it tried. This
> code has had **no independent security review**.

## What runs where

```text
your computer                      relay (Zavu)                  shared computer
Leon (client) ── wss ──────────►  pairs, forwards  ◄── wss ───── Leon (host service)
        └──────── Noise-encrypted application messages ────────────┘
```

The host offers exactly two primitives, over the encrypted channel:

* **run a command** (`Exec`): inherited environment plus the request's, no
  standard input, a time limit, output capped at 4 MiB per stream;
* **a terminal** (`PtyOpen`, `PtyData`, `PtyResize`, `PtyClose`, `PtyList`,
  `PtyAttach`): a pseudo-terminal owned by the host. It keeps running when
  every client disconnects. The host keeps the last 2 MiB of its output in a
  ring with a byte offset that only grows; a reconnecting client asks to
  continue `from_offset` and receives each later byte exactly once, or is told
  (`gap`) that the ring no longer reaches that far. Exited terminals stay
  listed for 30 minutes.

For Leon hosts it adds a third, opt-out-free but read-only primitive:

* **what this Leon knows** (`ShareState`, `ShareTranscript`): the projects of
  this machine and the sessions of its unified history — the same data this
  Leon's sidebar shows — plus each session's transcript by its
  `(agent, external_id)`. The host application answers from its own store;
  a headless `leon host` shares nothing and answers every request with an
  empty result. The client imports it as that machine's own state, so this
  machine's section of the client's sidebar mirrors the host's one: the same
  projects, the same history, resumable there.

Everything above those (git, probing, agents) is unchanged: a relay machine is
just another machine.

## Threat model

**What a paired device can do.** Run any command and open any terminal as the
user who runs the host, with that user's full rights, and — when the host is
Leon — read what that Leon holds of this machine: its projects and the sessions
of its unified history, all of which the terminal could read too. That is the
feature. There is no sandbox. Pair only your own devices. The host refuses to
start as root unless told `--allow-root`.

**What the relay can see.** Metadata: that a host with a given id is online, when
clients connect and leave, how many bytes flow and when, and the (public) room
part of a pairing code. It sees only ciphertext of the payload: not commands,
output, file names, keys or device names.

**What the relay can do.** Drop, delay or cut connections (availability). It
cannot read, forge or alter messages undetected (Noise authenticates every
message), cannot impersonate a host or a device (it lacks their keys), and cannot
guess a pairing code offline (SPAKE2). It cannot take over a host's id: the id is
a hash of the host's Ed25519 key and registration needs a signature over a fresh
nonce.

**A network attacker** has the relay's powers and less.

**A stolen or lost device.** It holds the private key of a paired device and can
reach the host until revoked. Revoke it (Share this machine, or
`leon host revoke`): the host drops its open sessions within about a second and
refuses it before answering the handshake. Keys are stored owner-only; they are
not encrypted at rest (a person who can read your files can read them), so full
disk encryption is advised.

**A compromised host or client computer.** Out of scope: it already has the user's
rights.

**Not protected.** Metadata (above); availability (the relay can refuse service,
and a host must be online); traffic analysis (message sizes and timing); a person
shoulder-reading the pairing code during its ten minutes (mitigated by single
use, five attempts, and the approval prompt that shows the new device's name and
fingerprint).

## Identity and key storage

Each installation has two long-term keys in its data directory (`identity.key`,
mode `0600` on Unix, the profile's default ACL on Windows, replaced atomically,
zeroized in memory, never logged or printed):

* an **X25519** static key, used by Noise. The **device id** is a fingerprint of
  its public key (grouped base32, e.g. `ABC-DEF-GHJ-KLM`);
* an **Ed25519** signing key, used only to prove to a relay that the host owns its
  **host id** (a hash of the verifying key).

The host keeps its paired devices in `host/devices.json` (owner-only): the
device's public key, name, first and last seen, revoked flag. A client stores the
host's id and pinned key in the machine record. After pairing, connections use
the pinned keys only.

## Protocol

### Rendezvous (peer ↔ relay)

WebSocket (`wss://` in production), binary messages. First byte: `1` control
(`postcard` message) or `2` data (`u32` channel number big-endian then payload, on
a host's connection; just the payload on a client's). Defined in
`leon-wire/src/relay.rs`.

* A host connects to `/v1/host`; the relay sends `Challenge{nonce, limits}`; the
  host answers `Register{verifying_key, signature, token}` where the signature
  covers `"leon-relay-register-v1" || nonce || verifying_key`; the relay derives
  the host id, verifies, answers `Registered`. A newer registration replaces an
  older one.
* A client connects to `/v1/join/<host id>` and sends `Join{token}`; the relay
  opens a channel to the host (`ChannelOpened`) and answers `Joined`. Either side
  leaving is announced (`ChannelClosed` / `PeerLeft`).
* For pairing, the host announces the public **room** of its code
  (`OpenPairing`); a client uses `/v1/pair/<ROOM>`.
* `token` is an opaque optional field reserved for a relay operator's accounts or
  plans; Leon sets it to nothing today.
* A relay may enforce message size, rates, byte budgets, clients per host, idle
  time and pairing attempts, and says its limits in `Challenge` / `Joined`.

### Pairing

The code is ten symbols of an alphabet without `0 1 I O`, shown `ABCD-EFG-HJK`:
four **room** symbols (public, routing only) and six **secret** symbols (30 bits).
A code lives ten minutes, works once and is burned by five failed attempts (also
counting attempts in flight, so parallel guessing cannot exceed five).

```text
client                                                      host
  | 1 SPAKE2 message A  --------------------------------------> |
  | <-------------------------------------  2 SPAKE2 message B  |  both hold K
  | 3 Noise XXpsk3 msg1: e  -----------------------------------> |
  | <----------------------------  4 msg2: e, ee, s, es          |
  | 5 msg3: s, se, psk(K)  ------------------------------------> |  host checks K
  | 6 transport: Hello{device name}  ============================> |
  |                          (host asks its owner, if configured) |
  | <==============  7 transport: Welcome{host name, verifying key}|
```

* **SPAKE2** (`spake2` 0.4, Ed25519 group; identities `leon-pair-client` /
  `leon-pair-host`) turns the secret symbols into a 32-byte key K that matches only
  if the codes match. Observers learn nothing that allows testing guesses offline.
* **Noise** `XXpsk3` (`snow` 0.10, `Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s`, prologue
  `leon/1/pair/<room>`) with K as the pre-shared key exchanges and authenticates
  both static keys. A wrong code fails at message 5, on the host, which counts it.
* Steps 6 and 7 travel under keys that depend on K. The client checks that the
  host id equals the hash of the verifying key the host announces.
* The relay learns the room and that a pairing happened; the host may refuse
  (`Approval`): headless hosts treat the code as the approval.

### Sessions

`Noise_IK_25519_ChaChaPoly_BLAKE2s` (prologue `leon/1/session`). The client knows
the host's static key (pinned at pairing) and sends its own encrypted in message 1;
the host reads it, checks its device list **before answering**, and sends nothing
to an unknown or revoked key. A replayed first message yields no session: the
attacker lacks the ephemeral key and cannot produce a valid transport message.

Each application message (a `leon-wire` frame: `u32` length, version byte,
`postcard` payload; at most 10 MiB) is one or more Noise transport messages,
fragmented to the 65535-byte Noise limit; a flag byte inside the encryption marks
more fragments and rekeying. Each side rekeys after 2^20 messages, 1 GiB or an
hour. Any authentication failure poisons the session: it is closed and refuses
everything after.

`postcard` was chosen over JSON (terminal bytes would need base64) and over a
schema compiler (a build step for a few dozen messages). Frames and messages are
bounded and decoding never panics (fuzz-style and truncation tests).

## Cryptography and dependencies

No home-made cryptography: Leon composes audited crates.

| Crate | Version | Used for | Licence |
| --- | --- | --- | --- |
| `snow` | 0.10.0 | Noise (ChaCha20-Poly1305, BLAKE2s, X25519) | Apache-2.0 OR MIT |
| `spake2` | 0.4.0 | the password-authenticated key exchange | MIT OR Apache-2.0 |
| `ed25519-dalek` | 2.2.0 | the relay-registration signature | BSD-3-Clause |
| `x25519-dalek` | 2.0.1 | deriving the public key of a stored secret | BSD-3-Clause |
| `subtle` | 2 | constant-time comparison of device keys | BSD-3-Clause |
| `zeroize` | 1 | wiping secrets | Apache-2.0 OR MIT |
| `sha2` | 0.10 | host and device ids | MIT OR Apache-2.0 |
| `postcard` | 1 | message encoding | MIT OR Apache-2.0 |
| `tokio-tungstenite` | 0.30 | the WebSocket transport (rustls, native roots) | MIT |

## Limits and defaults

| What | Value |
| --- | --- |
| Pairing code | 10 symbols, 30 secret bits, 10 minutes, single use, 5 failed attempts |
| Frame | 10 MiB; terminal chunk 32 KiB; command output 4 MiB per stream |
| Command time limit | 120 s default, 15 minutes at most |
| Terminal replay ring | 2 MiB per terminal; exited terminals kept 30 minutes |
| Per host | 16 sessions, 64 terminals, 8 commands at once per session |
| A session that cannot keep up | detached; the client reconnects and resumes |
| Reconnection | client and host back off from 1 s to 30 s |
| Rekey | 2^20 messages, 1 GiB or 1 hour |
| Shared state | at most 500 history sessions offered; transcripts read individually |

## Settings

`Relay server` (`remote_relay_url`, default `wss://relay.getleon.dev`), `Name of this
computer` (`remote_device_name`), `Share this machine` (`remote_share`) and `Ask
before pairing` (`remote_require_approval`). See [SETTINGS.md](SETTINGS.md).
`LEON_RELAY_URL` and `--relay` set the relay for `leon host`.

## Roadmap (not built)

* A background service that survives the window (launchd, systemd, a Windows
  service). Today sharing runs inside the open Leon and stops with it.
* A direct connection upgrade (hole punching) so the relay carries only the
  rendezvous.
* Accounts and plans on the relay (the opaque `token` field is reserved).
* A terminal still running on a relay machine listed in the tree after Leon
  restarts, attaching when opened (the protocol and `Client::pty_attach`
  support it).
* A live terminal the client keeps attached stays tied to its row while the
  host's own Leon keeps importing the same session, so no duplicate rows.
