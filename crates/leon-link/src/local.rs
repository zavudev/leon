//! The protocol on this computer's own socket, without the secure channel.
//!
//! A terminal keeper (see `leon_host::keeper`) is reached through a Unix
//! domain socket in a directory only its user can enter, and every connection
//! is checked against the user's id by the keeper. The file permissions are
//! the authentication, so there is no Noise handshake and no relay here: the
//! same [`Message`]s travel as plain frames, and the durable
//! [`Client`](crate::client::Client) (re-attach from the last offset, gaps,
//! reconnection) is reused as it is.
//!
//! * [`pipe_over`] turns any byte stream into a [`Pipe`] of whole frames;
//! * [`PlainChannel`] sends and receives [`Message`]s over such a pipe, as
//!   [`SecureChannel`](crate::channel::SecureChannel) does over a sealed one;
//! * [`UnixDialer`] (Unix only) dials a socket path, after checking it.
//!
//! **Whom a client talks to.** The keeper's socket may live in a predictable
//! directory (`/tmp/leon-keeper-<uid>` where there is no runtime directory),
//! which another user can create first and listen in; whatever a client then
//! sent (the whole environment of a new terminal, every keystroke) would be
//! theirs. So before it dials, a client checks the directory ([`judge_dir`]:
//! a real directory, not a link, owned by this user, closed to everybody
//! else; refused, never "fixed", when it is not) and after it connects, checks
//! that the process at the other end of the socket is this user's
//! ([`peer_allowed`]), before a single byte goes out. The keeper makes the same
//! checks of the directory it creates.

use leon_wire::{decode_frame, encode_frame, FrameError, Message, HEADER_LEN, MAX_FRAME_LEN};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::channel::LinkError;
use crate::pipe::{Pipe, PIPE_CAPACITY};

/// Messages over a [`Pipe`] with no encryption: one frame per pipe message.
#[derive(Debug)]
pub struct PlainChannel {
    pipe: Pipe,
}

impl PlainChannel {
    /// A channel over `pipe`, whose messages are whole frames.
    pub fn new(pipe: Pipe) -> Self {
        Self { pipe }
    }

    /// Sends a protocol message.
    pub async fn send(&mut self, message: &Message) -> Result<(), LinkError> {
        let frame = encode_frame(message)?;
        self.pipe.send(frame).await?;
        Ok(())
    }

    /// The next protocol message; `None` when the peer closed cleanly.
    /// Cancel-safe.
    pub async fn recv(&mut self) -> Result<Option<Message>, LinkError> {
        let Some(bytes) = self.pipe.recv().await else {
            return Ok(None);
        };
        let (message, used) = decode_frame(&bytes)?;
        if used != bytes.len() {
            return Err(FrameError::Malformed("trailing bytes".into()).into());
        }
        Ok(Some(message))
    }
}

impl PlainChannel {
    /// Splits the channel in two so that one task can write while another
    /// reads. A peer that both sends a flood and is sent one must read while
    /// it writes: two ends that each wait to write before they read, with the
    /// buffers between them full, wait for ever.
    pub fn split(self) -> (PlainSender, PlainReceiver) {
        (PlainSender(self.pipe.sender()), PlainReceiver(self.pipe))
    }
}

/// The sending half of a split [`PlainChannel`].
#[derive(Debug, Clone)]
pub struct PlainSender(mpsc::Sender<Vec<u8>>);

impl PlainSender {
    /// Sends a protocol message.
    pub async fn send(&self, message: &Message) -> Result<(), LinkError> {
        let frame = encode_frame(message)?;
        self.0.send(frame).await.map_err(|_| LinkError::Closed)
    }
}

/// The receiving half of a split [`PlainChannel`].
#[derive(Debug)]
pub struct PlainReceiver(Pipe);

impl PlainReceiver {
    /// The next protocol message; `None` when the peer closed cleanly.
    /// Cancel-safe.
    pub async fn recv(&mut self) -> Result<Option<Message>, LinkError> {
        let Some(bytes) = self.0.recv().await else {
            return Ok(None);
        };
        let (message, used) = decode_frame(&bytes)?;
        if used != bytes.len() {
            return Err(FrameError::Malformed("trailing bytes".into()).into());
        }
        Ok(Some(message))
    }
}

/// A [`Pipe`] over a byte stream: every pipe message is one frame, cut out of
/// the stream by its length header (which is refused when it announces more
/// than [`MAX_FRAME_LEN`]). Needs a Tokio runtime. The pipe closes when the
/// stream ends or a frame is refused; dropping it closes the stream.
pub fn pipe_over<S>(stream: S) -> Pipe
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if writer.write_all(&frame).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
        let _ = writer.shutdown().await;
    });
    tokio::spawn(async move {
        loop {
            let mut frame = vec![0u8; HEADER_LEN];
            if reader.read_exact(&mut frame).await.is_err() {
                break;
            }
            let length = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
            if length > MAX_FRAME_LEN {
                break;
            }
            // Grown as the bytes arrive, not reserved from a claimed length.
            let got = (&mut reader)
                .take(length as u64)
                .read_to_end(&mut frame)
                .await;
            if got.ok() != Some(length) || in_tx.send(frame).await.is_err() {
                break;
            }
        }
    });
    Pipe::new(out_tx, in_rx)
}

/// What `lstat` says of a directory a socket lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirFacts {
    /// It is a directory (and not what a link points to).
    pub is_dir: bool,
    /// It is itself a symbolic link.
    pub is_symlink: bool,
    /// The user that owns it.
    pub uid: u32,
    /// Its permission bits.
    pub mode: u32,
}

/// Why a socket's directory is not trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirRefusal {
    /// There is no such directory.
    NotThere,
    /// A symbolic link: somebody may have pointed it anywhere.
    Link,
    /// Not a directory.
    NotADirectory,
    /// Another user owns it, and so can put a socket of their own in it.
    OtherOwner,
    /// Others can enter or list it.
    TooOpen,
}

impl std::fmt::Display for DirRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DirRefusal::NotThere => "its directory does not exist",
            DirRefusal::Link => "its directory is a link",
            DirRefusal::NotADirectory => "its directory is not a directory",
            DirRefusal::OtherOwner => "its directory belongs to another user",
            DirRefusal::TooOpen => "its directory is open to other users",
        })
    }
}

/// Whether a directory may hold the keeper's socket for user `ours`: a real
/// directory, owned by them, with no permission for anybody else.
pub fn judge_dir(facts: DirFacts, ours: u32) -> Result<(), DirRefusal> {
    if facts.is_symlink {
        Err(DirRefusal::Link)
    } else if !facts.is_dir {
        Err(DirRefusal::NotADirectory)
    } else if facts.uid != ours {
        Err(DirRefusal::OtherOwner)
    } else if facts.mode & 0o077 != 0 {
        Err(DirRefusal::TooOpen)
    } else {
        Ok(())
    }
}

/// Whether a process of user `peer` may be the other end of a socket of user
/// `ours`: only the same user, and never one whose id could not be read.
pub fn peer_allowed(peer: Option<u32>, ours: u32) -> bool {
    peer == Some(ours)
}

/// This process's user id.
#[cfg(unix)]
pub fn our_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// [`judge_dir`] of what is on disk at `dir`, which is not followed if it is a
/// link.
#[cfg(unix)]
pub fn check_private_dir(dir: &std::path::Path, ours: u32) -> Result<(), DirRefusal> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = std::fs::symlink_metadata(dir).map_err(|_| DirRefusal::NotThere)?;
    judge_dir(
        DirFacts {
            is_dir: meta.is_dir(),
            is_symlink: meta.file_type().is_symlink(),
            uid: meta.uid(),
            mode: meta.permissions().mode(),
        },
        ours,
    )
}

/// The user id of the process at the other end of a connected Unix socket
/// (`SO_PEERCRED` on Linux, `getpeereid` on macOS); `None` when it cannot be
/// read.
#[cfg(unix)]
pub fn peer_uid(socket: &impl std::os::fd::AsRawFd) -> Option<u32> {
    let fd = socket.as_raw_fd();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `len` are valid for the call, and `fd` is a
        // socket that outlives it.
        let read = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                std::ptr::addr_of_mut!(cred).cast(),
                &mut len,
            )
        };
        (read == 0).then_some(cred.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let (mut uid, mut gid) = (0 as libc::uid_t, 0 as libc::gid_t);
        // SAFETY: both out-parameters are valid for the call.
        let read = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        (read == 0).then_some(uid)
    }
}

/// Checks the directory of `socket`, connects and checks that the other end is
/// this user's, without sending anything. Dropping the connection is the whole
/// of it: this is how a caller learns that a keeper answers, safely.
#[cfg(unix)]
pub fn probe_socket(socket: &std::path::Path) -> Result<(), String> {
    let ours = our_uid();
    let dir = socket.parent().ok_or("the socket has no directory")?;
    check_private_dir(dir, ours).map_err(|why| why.to_string())?;
    let stream = std::os::unix::net::UnixStream::connect(socket).map_err(|e| e.to_string())?;
    if peer_allowed(peer_uid(&stream), ours) {
        Ok(())
    } else {
        Err("the socket belongs to another user".to_owned())
    }
}

/// Dials a Unix domain socket, after checking its directory and, once
/// connected and before anything is sent, whose process is at the other end.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub struct UnixDialer {
    /// The socket's path.
    pub path: std::path::PathBuf,
}

#[cfg(unix)]
impl crate::client::Dialer for UnixDialer {
    fn dial(&self) -> crate::client::DialFuture {
        let path = self.path.clone();
        Box::pin(async move {
            let unreachable = |why: String| crate::relay_client::DialError::Unreachable(why);
            let ours = our_uid();
            let dir = path
                .parent()
                .ok_or_else(|| unreachable("the socket has no directory".into()))?;
            check_private_dir(dir, ours).map_err(|why| unreachable(why.to_string()))?;
            let stream = tokio::net::UnixStream::connect(&path)
                .await
                .map_err(|error| unreachable(error.to_string()))?;
            if !peer_allowed(peer_uid(&stream), ours) {
                return Err(unreachable("the socket belongs to another user".into()));
            }
            Ok(pipe_over(stream))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked() -> (PlainChannel, PlainChannel) {
        let (a, b) = tokio::io::duplex(1 << 16);
        (
            PlainChannel::new(pipe_over(a)),
            PlainChannel::new(pipe_over(b)),
        )
    }

    #[tokio::test]
    async fn messages_cross_a_byte_stream_whole_and_in_order() {
        let (mut a, mut b) = linked();
        a.send(&Message::Ping { nonce: 1 }).await.unwrap();
        a.send(&Message::PtyData {
            pty: 3,
            offset: 9,
            bytes: vec![7; 20_000],
        })
        .await
        .unwrap();
        assert_eq!(b.recv().await.unwrap(), Some(Message::Ping { nonce: 1 }));
        match b.recv().await.unwrap() {
            Some(Message::PtyData { pty, offset, bytes }) => {
                assert_eq!((pty, offset, bytes.len()), (3, 9, 20_000));
            }
            other => panic!("{other:?}"),
        }
        b.send(&Message::Pong { nonce: 1 }).await.unwrap();
        assert_eq!(a.recv().await.unwrap(), Some(Message::Pong { nonce: 1 }));
    }

    #[tokio::test]
    async fn a_frame_that_claims_more_than_the_limit_closes_the_pipe() {
        let (mut raw, other) = tokio::io::duplex(1 << 10);
        let mut channel = PlainChannel::new(pipe_over(other));
        let mut header = ((MAX_FRAME_LEN as u32) + 1).to_be_bytes().to_vec();
        header.push(2);
        raw.write_all(&header).await.unwrap();
        assert_eq!(channel.recv().await.unwrap(), None);
    }

    #[tokio::test]
    async fn garbage_inside_a_frame_is_an_error_not_a_panic() {
        let (mut raw, other) = tokio::io::duplex(1 << 10);
        let mut channel = PlainChannel::new(pipe_over(other));
        let mut frame = 3u32.to_be_bytes().to_vec();
        frame.push(2);
        frame.extend([0xff, 0xff, 0xff]);
        raw.write_all(&frame).await.unwrap();
        assert!(channel.recv().await.is_err());
    }

    fn facts() -> DirFacts {
        DirFacts {
            is_dir: true,
            is_symlink: false,
            uid: 1000,
            mode: 0o700,
        }
    }

    #[test]
    fn only_a_real_directory_of_ours_closed_to_others_may_hold_the_socket() {
        assert_eq!(judge_dir(facts(), 1000), Ok(()));
        assert_eq!(
            judge_dir(
                DirFacts {
                    uid: 1001,
                    ..facts()
                },
                1000
            ),
            Err(DirRefusal::OtherOwner)
        );
        for mode in [0o755, 0o750, 0o705, 0o777, 0o701] {
            assert_eq!(
                judge_dir(DirFacts { mode, ..facts() }, 1000),
                Err(DirRefusal::TooOpen),
                "{mode:o}"
            );
        }
        // Only the owner's bits do not matter.
        assert_eq!(
            judge_dir(
                DirFacts {
                    mode: 0o500,
                    ..facts()
                },
                1000
            ),
            Ok(())
        );
        assert_eq!(
            judge_dir(
                DirFacts {
                    is_symlink: true,
                    ..facts()
                },
                1000
            ),
            Err(DirRefusal::Link),
            "a link is refused whatever it looks like"
        );
        assert_eq!(
            judge_dir(
                DirFacts {
                    is_dir: false,
                    ..facts()
                },
                1000
            ),
            Err(DirRefusal::NotADirectory)
        );
        // Not even root's directory is ours.
        assert_eq!(
            judge_dir(DirFacts { uid: 0, ..facts() }, 1000),
            Err(DirRefusal::OtherOwner)
        );
    }

    #[test]
    fn a_peer_is_ours_only_when_its_id_is_read_and_equal() {
        assert!(peer_allowed(Some(1000), 1000));
        assert!(!peer_allowed(Some(0), 1000));
        assert!(!peer_allowed(None, 1000));
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_on_disk_is_judged_without_following_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let ours = our_uid();
        let dir = base.path().join("closed");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(check_private_dir(&dir, ours), Ok(()));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(check_private_dir(&dir, ours), Err(DirRefusal::TooOpen));
        assert_eq!(
            check_private_dir(&dir, ours + 1),
            Err(DirRefusal::OtherOwner)
        );
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert_eq!(check_private_dir(&link, ours), Err(DirRefusal::Link));
        assert_eq!(
            check_private_dir(&base.path().join("nowhere"), ours),
            Err(DirRefusal::NotThere)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_peer_of_a_socket_pair_is_this_user() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a), Some(our_uid()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_socket_in_a_directory_that_is_not_ours_to_trust_is_never_connected_to() {
        use crate::client::Dialer;
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("k");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let socket = dir.join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let dialer = UnixDialer {
            path: socket.clone(),
        };
        // Open to others: refused, and the listener never saw a connection.
        assert!(dialer.dial().await.is_err());
        assert!(probe_socket(&socket).is_err());
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "nothing connected"
        );
        // A link to a good directory is refused too.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = base.path().join("l");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(UnixDialer {
            path: link.join("s.sock")
        }
        .dial()
        .await
        .is_err());
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        // The directory as it should be: connected, and the peer is ours.
        assert!(dialer.dial().await.is_ok());
        assert!(probe_socket(&socket).is_ok());
    }

    #[tokio::test]
    async fn the_end_of_the_stream_ends_the_channel() {
        let (a, mut b) = linked();
        drop(a);
        assert_eq!(b.recv().await.unwrap(), None);
    }
}
