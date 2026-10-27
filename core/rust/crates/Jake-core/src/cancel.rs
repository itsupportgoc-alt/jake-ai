//! Cooperative cancellation shared by the runtime, tools and speech output.
//!
//! The emergency stop is enforced **here**, in the Rust layer: it does not depend on
//! the model, the brain process or any AI output being responsive.

use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone, Debug)]
pub struct CancelFlag {
    inner: Arc<watch::Sender<bool>>,
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelFlag {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(watch::Sender::new(false)),
        }
    }

    /// Trip the stop. Idempotent; wakes every waiter immediately.
    pub fn cancel(&self, reason: &str) {
        tracing::warn!(%reason, "emergency stop / cancellation triggered");
        self.inner.send_replace(true);
    }

    /// Reset so the next turn can run (the user must re-arm after a stop).
    pub fn reset(&self) {
        self.inner.send_replace(false);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.inner.borrow()
    }

    /// Returns `Err(Cancelled)` if already tripped.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// Resolves the moment the flag trips (immediately if it already has).
    pub async fn cancelled(&self) {
        let mut rx = self.inner.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }

    /// `true` if the flag tripped within `dur`, `false` if the deadline passed.
    pub async fn wait_or_timeout(&self, dur: std::time::Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }
        let flag = self.clone();
        tokio::select! {
            _ = flag.cancelled() => true,
            _ = tokio::time::sleep(dur) => false,
        }
    }
}

/// Error returned when an operation observes the cancellation flag.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("operation cancelled")]
pub struct Cancelled;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn flag_starts_clear_and_trips_once() {
        let f = CancelFlag::new();
        assert!(!f.is_cancelled());
        assert!(f.check().is_ok());
        f.cancel("test");
        assert!(f.is_cancelled());
        assert!(f.check().is_err());
        f.cancel("again"); // idempotent
        assert!(f.is_cancelled());
        f.reset();
        assert!(!f.is_cancelled());
    }

    #[tokio::test]
    async fn waiters_are_woken_by_cancel() {
        let f = CancelFlag::new();
        let waiter = f.clone();
        let handle = tokio::spawn(async move {
            waiter.cancelled().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        f.cancel("stop");
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("waiter never woke")
            .unwrap();
    }

    #[tokio::test]
    async fn already_cancelled_wait_returns_immediately() {
        let f = CancelFlag::new();
        f.cancel("now");
        tokio::time::timeout(std::time::Duration::from_millis(100), f.cancelled())
            .await
            .expect("should be immediate");
    }

    #[tokio::test]
    async fn wait_or_timeout_reports_false_on_timeout() {
        let f = CancelFlag::new();
        assert!(!f.wait_or_timeout(std::time::Duration::from_millis(10)).await);
        f.cancel("x");
        assert!(f.wait_or_timeout(std::time::Duration::from_millis(10)).await);
    }
}