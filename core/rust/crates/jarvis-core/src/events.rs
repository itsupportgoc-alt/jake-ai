//! Process-wide event bus.
//!
//! A `tokio::sync::broadcast` of `Arc<Event>`: subscribers (CLI, future UI, tests)
//! observe the same stream the runtime publishes, and every event is mirrored into
//! the structured log so history survives a restart.

use jarvis_protocol::event::{Event, EventKind};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<Event>>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(16));
        Self { tx }
    }

    /// Publish an event: log it (at its own severity) and fan it out to subscribers.
    pub fn publish(&self, event: Event) {
        let correlation = event
            .correlation_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".into());
        let data = if event.data.is_null() {
            None
        } else {
            Some(event.data.to_string())
        };
        match event.kind {
            EventKind::Error => tracing::error!(
                correlation = %correlation,
                kind = ?event.kind,
                data = data.as_deref().unwrap_or("-"),
                "{}", event.message
            ),
            EventKind::Warning => tracing::warn!(
                correlation = %correlation,
                kind = ?event.kind,
                data = data.as_deref().unwrap_or("-"),
                "{}", event.message
            ),
            _ => tracing::info!(
                correlation = %correlation,
                kind = ?event.kind,
                data = data.as_deref().unwrap_or("-"),
                "{}", event.message
            ),
        }
        let _ = self.tx.send(Arc::new(event));
    }

    /// Convenience: publish with kind + message.
    pub fn emit(&self, kind: EventKind, message: impl Into<String>) {
        self.publish(Event::new(kind, message));
    }

    /// New subscriber. Lagging subscribers skip old events instead of blocking the
    /// runtime — the bus must never stall the agent.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        self.tx.subscribe()
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_events() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        bus.emit(EventKind::TaskStarted, "task started");
        let got = rx.recv().await.unwrap();
        assert_eq!(got.message, "task started");
        assert_eq!(got.kind, EventKind::TaskStarted);
    }

    #[tokio::test]
    async fn multiple_subscribers_each_get_everything() {
        let bus = EventBus::default();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.emit(EventKind::ToolRequested, "one");
        bus.emit(EventKind::ToolCompleted, "two");
        assert_eq!(a.recv().await.unwrap().message, "one");
        assert_eq!(a.recv().await.unwrap().message, "two");
        assert_eq!(b.recv().await.unwrap().message, "one");
        assert_eq!(b.recv().await.unwrap().message, "two");
    }

    #[tokio::test]
    async fn a_slow_subscriber_never_blocks_the_runtime() {
        let bus = EventBus::new(16);
        let mut slow = bus.subscribe();
        for i in 0..64 {
            bus.emit(EventKind::AgentStep, format!("step {i}"));
        }
        // runtime is not blocked even though `slow` never read anything
        assert_eq!(bus.subscriber_count(), 1);
        // and it can still catch up (possibly skipping, never panicking)
        loop {
            match slow.try_recv() {
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    }

    #[tokio::test]
    async fn correlation_id_travels_with_the_event() {
        use uuid::Uuid;
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let id = Uuid::new_v4();
        bus.publish(Event::new(EventKind::UserMessage, "hi").with_correlation(id));
        assert_eq!(rx.recv().await.unwrap().correlation_id, Some(id));
    }
}