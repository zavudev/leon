//! A transport-independent, ordered pipe of messages.
//!
//! Everything in this crate that needs to move bytes does it through a
//! [`Pipe`]: one end sends whole messages, the other receives them in order.
//! The relay adapter, an in-memory pair for tests, or any other transport
//! can sit behind one. Dropping an end closes the other: `recv` then returns
//! `None`.

use tokio::sync::mpsc;

/// How many messages a pipe buffers before `send` waits.
pub const PIPE_CAPACITY: usize = 64;

/// The peer is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeClosed;

impl std::fmt::Display for PipeClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the connection is closed")
    }
}

impl std::error::Error for PipeClosed {}

/// One end of a message pipe.
#[derive(Debug)]
pub struct Pipe {
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
}

impl Pipe {
    /// A pipe from its two halves.
    pub fn new(tx: mpsc::Sender<Vec<u8>>, rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self { tx, rx }
    }

    /// Two connected ends.
    pub fn pair() -> (Pipe, Pipe) {
        let (a_tx, b_rx) = mpsc::channel(PIPE_CAPACITY);
        let (b_tx, a_rx) = mpsc::channel(PIPE_CAPACITY);
        (Pipe::new(a_tx, a_rx), Pipe::new(b_tx, b_rx))
    }

    /// Sends a message.
    pub async fn send(&self, message: Vec<u8>) -> Result<(), PipeClosed> {
        self.tx.send(message).await.map_err(|_| PipeClosed)
    }

    /// The next message, or `None` once the other end is gone and everything
    /// it sent has been read. Cancel-safe.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }

    /// A handle that can send while this end is being read elsewhere.
    pub fn sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.tx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn messages_arrive_whole_and_in_order() {
        let (a, mut b) = Pipe::pair();
        a.send(b"one".to_vec()).await.unwrap();
        a.send(b"two".to_vec()).await.unwrap();
        assert_eq!(b.recv().await.unwrap(), b"one");
        assert_eq!(b.recv().await.unwrap(), b"two");
    }

    #[tokio::test]
    async fn dropping_one_end_closes_the_other() {
        let (a, mut b) = Pipe::pair();
        a.send(b"last".to_vec()).await.unwrap();
        drop(a);
        assert_eq!(b.recv().await.unwrap(), b"last");
        assert_eq!(b.recv().await, None);
        assert_eq!(b.send(vec![1]).await, Err(PipeClosed));
    }
}
