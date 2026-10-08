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

* **run a command** (`Exec`): inherited environment plus the request's, the
  request's standard input if it carries any (at most 4 MiB, written while the
  output is read and closed after it; none otherwise, so the command sees end
  of file), a time limit, output capped at 4 MiB per stream;
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

Files are no exception. The engine reads, saves and lists a file on any machine
with a short POSIX `sh` script run as an `Exec` (or over SSH), and `std::fs` on
the local one; both answer in the same shapes. Every script prints a
`LEON-FILE 1` line first, so a login banner before it is skipped, and takes the
path as an argument, never inside its text. A read is capped at 2 MiB, reports a
binary file instead of sending it and returns the `cksum` of the bytes as the
file's revision. A save sends the new bytes as the command's standard input,
checks in the shell that the revision is still the one that was read (otherwise
it prints `CONFLICT` and writes nothing), writes a temporary file next to the
file with its mode and renames it over it. Folders are listed one level at a
time, the files of a project come from `git ls-files` (a bounded `find`
outside a repository) and the git marks from `git status`. For a folder inside
a larger repository, `git status` names paths from the work tree's root, so the
script prints `git rev-parse --show-prefix` on the line after `GIT` and the
marks are made relative to the folder by that prefix. The remote machine needs
`sh`, `cksum`, `base64` and `wc`; none of this works on a remote Windows host.

**Searching the text of a project** is one more script of the same kind
(`leon_remote::search`), run as one execute that the engine gives up after
30 s. The root, the query and two switches (`F` plain or `E` regular
expression, `i` or `s` for case) are its arguments `$1` to `$4`; the query is
passed after `-e`, so it is never part of the script's text and a leading dash
is only text. It uses the first tool the machine has, and prints its name after
the marker:

| Tool | Command | Output read |
| --- | --- | --- |
| `RG` | `rg --null --line-number --column --hidden -g '!.git' --max-filesize 1M` | `path NUL line:col:text` |
| `GIT` | `git grep -n -I -z --column --untracked --exclude-standard` (inside a work tree) | `path NUL line NUL col NUL text` |
| `GREP` | `grep -rnI --null --exclude-dir=.git` and the usual dependency and build folders | `path NUL line:text` (`path:line:text` for a grep without `--null`) |

Each is cut by `head -c` at 3.5 MB, below the 4 MiB `MAX_EXEC_OUTPUT`, and a
line cut by it is dropped. The parsers turn the three formats into
`SearchHit { path, line, col, text }`, at most 1000 of them with a `truncated`
flag, text clipped at 400 characters; paths that leave the root are ignored. A
regular expression is the tool's own dialect (POSIX extended for `git grep` and
`grep`); a tool that finds nothing and complains on standard error (a pattern
that is none) is reported with its last line. `git grep` needs git 2.19 for
`--column`.

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

The version byte of every frame is the protocol version, and `Hello` / `Welcome`
carry it too. It is **2** since `Exec` can carry standard input (the `stdin` field
of its command, a change to the message layout that `postcard` cannot read around).
A peer speaking 1.x and one speaking 2.x refuse each other at the version check:
the frame is rejected and the connection closes, and the client reports a version
mismatch instead of misreading messages. Update both ends together.

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

## The host as a background service

`leon host` runs in the foreground, and **Share this machine** runs the host
inside the open Leon: both stop with their window or terminal. `leon host
service` installs the same host as a service of your own session, so a computer
stays shared with no window open:

```text
leon host service install [--linger] [--relay <url>] [--name <name>] [--data-dir <path>]
leon host service status
leon host service logs [--lines <n>]
leon host service uninstall
```

* **Linux.** `install` writes the systemd user unit `leon-host.service` under
  `$XDG_CONFIG_HOME/systemd/user` (`~/.config/systemd/user`), then runs
  `systemctl --user daemon-reload`, `enable` and `restart`, waits two seconds
  and asks systemd whether the host is running. If it is not (it started and
  ended, or waits to be restarted) `install` says so, points to `logs` and
  exits with status 1, leaving the unit in place; it does not report success
  for a unit that is failing. A host that ends later than those two seconds is
  not caught: `status` shows it. `logs` reads the user journal
  (`journalctl --user --unit leon-host.service`).
* **macOS.** `install` writes the launchd agent `dev.zavu.leon.host.plist` in
  `~/Library/LaunchAgents`, loads it with `launchctl bootstrap gui/<uid>` and
  checks the same way with `launchctl print`. `logs` shows the end of `~/Library/Logs/Leon/host.log`.
* **Windows.** Not supported yet: the command says so and exits with status 1.
  Run `leon host` in a terminal.

What the service is:

* **Your rights, never root.** It is a unit of your own user manager and a
  launchd agent of your own session; the commands refuse to run as root, and
  there is no `--allow-root` for them. A paired device gets what you have, as
  with any host.
* **The program that installed it.** The unit runs `leon host --data-dir <dir>`
  (plus `--relay` and `--name` when given at `install`; the `LEON_RELAY_URL` of
  the installing session counts as a relay) with the file that ran `install`,
  at the path the operating system reports for it (a link is followed). When
  an update replaces that file at the same path, the service runs the new
  version the next time it starts (after a failure, a reboot or another
  `leon host service install`); it does not restart itself when Leon updates,
  since that would hang up the terminals being served. A package manager that
  puts each version in a folder of its own (Homebrew's `Cellar`, Nix's store)
  does not do that: the path written is the old version's, which is removed
  sooner or later. `install` prints a note when it sees such a path; run it
  again after each update there. Leon refuses to install
  from a place that is gone later (an AppImage, a temporary folder, a
  translocated application, a disk image). The installing session's `PATH` is
  written into the definition, because the manager starts the host with a short
  one.
* **It restarts after a failure.** systemd: `Restart=on-failure`, 5 seconds at
  first and longer each time (six steps up to 5 minutes; `RestartSteps` needs
  systemd 254, an older systemd restarts every 5 seconds), with no limit on the
  number of restarts. launchd: `KeepAlive` on a non-zero exit, at least 30
  seconds apart; it has no growing delay. A host that was killed (`SIGKILL`,
  out of memory) leaves its heartbeat file behind; the next start ignores it,
  because a heartbeat counts only while it is under ten seconds old **and** its
  process still exists, so the restart is not refused for a predecessor that is
  gone (the one case left is a process id that the system has already given to
  an unrelated program). A `leon host` that is really running in a terminal for
  the same data directory makes the service exit with a failure and wait for
  its turn; it takes over when the terminal one stops. **Share this machine**
  in the app writes no heartbeat, so nothing keeps the service and the app's
  sharing from running at once on one computer: use one of them. Stopping
  with `systemctl --user stop`, `launchctl bootout` or Ctrl-C ends the host
  cleanly (it answers `SIGTERM` as well as `SIGINT`; a test sends the signal to
  the host's shutdown wait, but it has not been run under a real systemd or
  launchd).
* **Lingering (Linux).** A user's systemd manager normally stops at the last
  logout and starts at the next login, taking the service with it. `install
  --linger` also runs `loginctl enable-linger` for your own user (no root on
  systemd distributions that allow it; if yours asks for authentication,
  `install` says so and the service stays installed without it). Without the
  flag nothing about lingering is changed, and `install` prints this
  explanation. `uninstall` never turns lingering off (it may have been on
  before); `loginctl disable-linger` does. macOS agents start at login.
* **`status`** says whether the unit or agent is installed, whether it runs
  (the manager's word) and since when (the host's own heartbeat; the
  manager's timestamp without one), its relay and host id, whether it starts at
  login or at boot, and how many devices are paired. It exits 0 only when the
  service is running. On Linux "starts at login" is systemd's `enabled`; on
  macOS it is asked of `launchctl print-disabled`, and `status` says when that
  could not be checked.
* **`uninstall`** stops the service (`systemctl --user disable --now`,
  `launchctl bootout`) and, only when it is known not to be running any more,
  removes its definition. If the stop fails and the manager still reports the
  service running or waiting to restart, or cannot be asked, `uninstall`
  prints why, keeps the definition so the service is not left running with no
  unit to find it by, and exits with status 1: fix the cause and run it again.
  The pairings (`host/devices.json`), the identity, the settings and the macOS
  log stay.

Limits. It does not make the **desktop app's own local sessions** survive the
window: those terminals belong to the app's process, and only the terminals the
host serves to a paired device live in the service. The app does not yet list
and re-attach a relay machine's still-running terminals after it restarts (the
protocol supports it). `logs` shows the end of the log, not a live follow (the
command it prints for that is `journalctl --user --follow` or `tail -f`). The
Linux and macOS sides are covered by tests that script the answers of
`systemctl`, `launchctl` and `loginctl` and compare the generated text; no real
systemd or launchd was driven by those tests (the unit text was checked once, by
hand, with `systemd-analyze --user verify` on systemd 261, which accepted it),
and the macOS side has not been run on a Mac here.

## The keeper of local sessions is not the host

With the setting `durable_sessions`, Leon holds the terminals of this computer's
own sessions in `leon keeper`, a background process that outlives the window.
It looks like a host (it reuses the host's terminal table and replay ring and
speaks the same terminal messages) and is a different thing:

| | the host (`leon host`, `leon host service`, Share this machine) | the keeper (`leon keeper`) |
| --- | --- | --- |
| Serves | devices you paired, through the relay | this user's own Leon window, on this computer |
| Reached by | the relay, end-to-end encrypted (Noise) | a Unix socket in a 0700 directory; the keeper checks every peer's uid and the window checks the directory (a real directory of its own, closed to others) and the keeper's uid before sending anything; no encryption, no relay, no pairing |
| Identity | its keys and the device registry in the data directory | none; file permissions are the authentication |
| Started by | you, or the service | the window, on demand; ends by itself when idle |
| Terminals | for the devices; run as the host's user | for the window; run as the same user |

They are separate processes and share nothing but the code of the terminal
table: the keeper does not read or write the data directory's keys or registry
and the host does not open the keeper's socket, so they do not contend. A
computer can run both; a session opened here is held by the keeper, and a
session a paired device opens is held by the host. The keeper does not accept
`Exec` or the sharing requests, only terminals. It uses the same messages, and
adds three at the end (`PtyProbe`, `PtyProbed`, `PtyTerminate`: who is in front
of a terminal, which a client that does not hold the pseudo-terminal cannot see
for itself); the host answers them too, so a relay machine can list its
terminals, attach after a restart and read their foreground with the same code,
but a client does not send them to a host it has not checked knows them (an
older host drops a connection that sends a message it does not know), and Leon
does not yet list or re-attach a relay machine's terminals after a restart.

## Settings

`Relay server` (`remote_relay_url`, default `wss://relay.getleon.dev`), `Name of this
computer` (`remote_device_name`), `Share this machine` (`remote_share`) and `Ask
before pairing` (`remote_require_approval`). See [SETTINGS.md](SETTINGS.md).
`LEON_RELAY_URL` and `--relay` set the relay for `leon host`.

## Roadmap (not built)

* A Windows service (the background service is for Linux and macOS).
* A direct connection upgrade (hole punching) so the relay carries only the
  rendezvous.
* Accounts and plans on the relay (the opaque `token` field is reserved).
* A terminal still running on a relay machine listed in the tree after Leon
  restarts, attaching when opened (the protocol and `Client::pty_attach`
  support it).
* A live terminal the client keeps attached stays tied to its row while the
  host's own Leon keeps importing the same session, so no duplicate rows.
