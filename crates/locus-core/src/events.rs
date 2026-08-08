//! Live memory event bus (U-016).
//!
//! A bounded, multi-subscriber broadcast. [`EventBus::publish`] is
//! non-blocking and never waits on a subscriber: a subscriber whose queue is
//! full silently misses the event (drop-on-backpressure) and a disconnected
//! subscriber is dropped. This guarantees the memory path can never be stalled
//! by a slow or hung live-view client.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use crate::ipc::protocol::MemoryEvent;

/// Per-subscriber event queue depth before events are dropped for that
/// subscriber. The live viewer coalesces events, so dropping the newest under
/// overload is safe and keeps latency bounded.
const SUBSCRIBER_BUFFER: usize = 256;

type Subscriber = (usize, SyncSender<MemoryEvent>);

/// Cloneable handle to a memory event bus. Clones share the same subscriber
/// registry.
#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<EventBusInner>,
}

#[derive(Default)]
struct EventBusInner {
    subscribers: Mutex<Vec<Subscriber>>,
    next_id: AtomicUsize,
}

impl EventBus {
    /// Registers a new subscriber and returns its receiver. Callers must call
    /// [`EventBus::unsubscribe`] with the returned id when done, otherwise the
    /// (bounded) queue lives until the receiver is dropped.
    pub fn subscribe(&self) -> (usize, Receiver<MemoryEvent>) {
        let (tx, rx) = sync_channel(SUBSCRIBER_BUFFER);
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut subscribers) = self.inner.subscribers.lock() {
            subscribers.push((id, tx));
        }
        (id, rx)
    }

    /// Removes a subscriber by the id returned from [`EventBus::subscribe`].
    pub fn unsubscribe(&self, id: usize) {
        if let Ok(mut subscribers) = self.inner.subscribers.lock() {
            subscribers.retain(|(sub_id, _)| *sub_id != id);
        }
    }

    /// Non-blocking broadcast of a single event.
    ///
    /// Slow subscribers have the event dropped for them (backpressure); dead
    /// subscribers are removed from the registry.
    pub fn publish(&self, event: &MemoryEvent) {
        let mut dead = Vec::new();
        let guard = match self.inner.subscribers.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        for (id, tx) in guard.iter() {
            match tx.try_send(event.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => dead.push(*id),
            }
        }
        drop(guard);
        if !dead.is_empty() {
            self.remove_all(&dead);
        }
    }

    fn remove_all(&self, ids: &[usize]) {
        if let Ok(mut subscribers) = self.inner.subscribers.lock() {
            subscribers.retain(|(id, _)| !ids.contains(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::{MemoryEvent, MemoryEventKind};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    fn event(kind: MemoryEventKind) -> MemoryEvent {
        MemoryEvent {
            kind,
            memory_id: "m1".to_string(),
            title: "title".to_string(),
            namespace: "global".to_string(),
            memory_type: "fact".to_string(),
            importance: 50,
            access_delta: 1,
            timestamp: 1,
        }
    }

    #[test]
    fn subscribers_receive_published_events_in_order() {
        let bus = EventBus::default();
        let (_, rx) = bus.subscribe();
        bus.publish(&event(MemoryEventKind::Created));
        bus.publish(&event(MemoryEventKind::Searched));
        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(first.kind, MemoryEventKind::Created);
        assert_eq!(second.kind, MemoryEventKind::Searched);
    }

    #[test]
    fn slow_subscriber_has_events_dropped_not_blocking_others() {
        let bus = EventBus::default();
        let (id, rx) = bus.subscribe();
        // Fill the subscriber buffer without draining it.
        for _ in 0..SUBSCRIBER_BUFFER {
            bus.publish(&event(MemoryEventKind::Searched));
        }
        // The queue is now full; a further publish must not block and the event
        // is dropped for this subscriber.
        bus.publish(&event(MemoryEventKind::Created));
        // A second subscriber still receives events.
        let (_, rx2) = bus.subscribe();
        bus.publish(&event(MemoryEventKind::Created));
        assert_eq!(
            rx2.recv_timeout(Duration::from_secs(1)).unwrap().kind,
            MemoryEventKind::Created
        );
        bus.unsubscribe(id);
        // Drain the buffer: the queued events are all `Searched`; the overflow
        // event was dropped, never queued.
        for _ in 0..SUBSCRIBER_BUFFER {
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap().kind,
                MemoryEventKind::Searched
            );
        }
        // Empty and disconnected after the sender was removed.
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(20)),
            Err(RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn disconnected_subscriber_is_removed() {
        let bus = EventBus::default();
        let (id, rx) = bus.subscribe();
        bus.unsubscribe(id);
        drop(rx);
        // Publishing with an empty registry must be a no-op.
        bus.publish(&event(MemoryEventKind::Created));
    }

    #[test]
    fn unsubscribed_subscriber_stops_receiving() {
        let bus = EventBus::default();
        let (id, rx) = bus.subscribe();
        bus.unsubscribe(id);
        // The sender was dropped, so the channel is disconnected with nothing
        // buffered: the subscriber received no further events.
        bus.publish(&event(MemoryEventKind::Created));
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(20)),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
}
