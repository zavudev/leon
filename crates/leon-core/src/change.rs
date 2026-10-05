//! Change notifications emitted by the store.
//!
//! The store tells interested parties *that* something changed, never *what*
//! the new data is. A listener reacts by re-reading whatever it shows. This
//! keeps the channel tiny, makes a dropped notification harmless (the listener
//! is simply told to refresh everything) and means there is exactly one source
//! of truth: the database.

use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

/// How many notifications may queue up for a slow listener before it is told
/// to refresh everything instead.
pub(crate) const CHANGE_CAPACITY: usize = 256;

/// The area of the store a write touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreChange {
    /// Machines were added, changed or removed.
    Machines,
    /// Projects were added, changed or removed.
    Projects,
    /// Worktrees were added, changed or removed.
    Worktrees,
    /// Sessions or their messages were added, changed or removed.
    Sessions,
    /// Usage readings or their history changed.
    Usage,
    /// Anything may have changed; re-read everything that is displayed.
    Everything,
}

/// A subscription to the store's change notifications.
///
/// Obtained from [`Store::subscribe`](crate::Store::subscribe). Only changes
/// made after the subscription was created are reported.
#[derive(Debug)]
pub struct ChangeListener {
    receiver: broadcast::Receiver<StoreChange>,
}

impl ChangeListener {
    pub(crate) fn new(receiver: broadcast::Receiver<StoreChange>) -> Self {
        Self { receiver }
    }

    /// Waits for the next change. Returns `None` once the store has been
    /// dropped and no notification is left.
    ///
    /// A listener that fell too far behind receives
    /// [`StoreChange::Everything`] in place of the notifications it missed.
    pub async fn next(&mut self) -> Option<StoreChange> {
        match self.receiver.recv().await {
            Ok(change) => Some(change),
            Err(RecvError::Lagged(_)) => Some(StoreChange::Everything),
            Err(RecvError::Closed) => None,
        }
    }

    /// Returns a pending change without waiting, or `None` when nothing is
    /// pending.
    pub fn try_next(&mut self) -> Option<StoreChange> {
        match self.receiver.try_recv() {
            Ok(change) => Some(change),
            Err(TryRecvError::Lagged(_)) => Some(StoreChange::Everything),
            Err(TryRecvError::Empty | TryRecvError::Closed) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listener_that_falls_behind_is_told_to_refresh_everything() {
        let (sender, receiver) = broadcast::channel(2);
        let mut listener = ChangeListener::new(receiver);
        for _ in 0..5 {
            sender.send(StoreChange::Sessions).unwrap();
        }
        assert_eq!(listener.try_next(), Some(StoreChange::Everything));
        assert_eq!(listener.try_next(), Some(StoreChange::Sessions));
    }

    #[test]
    fn an_idle_listener_reports_nothing() {
        let (_sender, receiver) = broadcast::channel::<StoreChange>(2);
        assert_eq!(ChangeListener::new(receiver).try_next(), None);
    }

    #[tokio::test]
    async fn waiting_ends_when_the_sender_is_gone() {
        let (sender, receiver) = broadcast::channel(2);
        let mut listener = ChangeListener::new(receiver);
        sender.send(StoreChange::Machines).unwrap();
        drop(sender);
        assert_eq!(listener.next().await, Some(StoreChange::Machines));
        assert_eq!(listener.next().await, None);
    }
}
