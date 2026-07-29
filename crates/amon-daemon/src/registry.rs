//! Who is connected, and who wants to be told about it.
//!
//! An entry exists exactly as long as the wrapper connection that registered
//! it. There is no persistence and no expiry: a wrapper that dies takes its
//! entry with it, which is what makes the registry self-healing after a crash
//! or a `kill -9` (ADR-0002).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use amon_protocol::{AgentEntry, AgentPatch, Event};

/// Identifies one connection. Registry entries and subscriptions are keyed by
/// this so that dropping a connection cleans up everything it owned.
pub type ConnectionId = u64;

#[derive(Default)]
struct Inner {
    agents: HashMap<ConnectionId, AgentEntry>,
    subscribers: HashMap<ConnectionId, std::sync::mpsc::Sender<Event>>,
}

#[derive(Clone, Default)]
pub struct Registry {
    inner: Arc<Mutex<Inner>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Claims or replaces this connection's entry. A wrapper re-registers
    /// after reconnecting, so seeing the same id twice is normal.
    pub fn register(&self, connection: ConnectionId, entry: AgentEntry) {
        let mut inner = self.lock();
        let existing = inner.agents.insert(connection, entry.clone());
        let event = match existing {
            Some(previous) if previous.id == entry.id => Event::AgentUpdated(entry),
            _ => Event::AgentConnected(entry),
        };
        Self::broadcast(&mut inner, event);
    }

    /// Applies a patch to this connection's entry. Patches from a connection
    /// that never registered are ignored — a wrapper cannot edit anyone else's
    /// entry, because the connection is the identity.
    pub fn update(&self, connection: ConnectionId, patch: &AgentPatch) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.agents.get_mut(&connection) else {
            return false;
        };
        if !patch.apply(entry) {
            return true;
        }
        let event = Event::AgentUpdated(entry.clone());
        Self::broadcast(&mut inner, event);
        true
    }

    /// Forgets everything a connection owned. Called for every disconnect,
    /// whatever the reason.
    pub fn disconnect(&self, connection: ConnectionId) {
        let mut inner = self.lock();
        inner.subscribers.remove(&connection);
        if let Some(entry) = inner.agents.remove(&connection) {
            Self::broadcast(&mut inner, Event::AgentDisconnected { id: entry.id });
        }
    }

    pub fn subscribe(&self, connection: ConnectionId, events: std::sync::mpsc::Sender<Event>) {
        self.lock().subscribers.insert(connection, events);
    }

    /// Every connected agent, most urgent first: the question `amon status`
    /// answers is "does anything need me?".
    pub fn agents(&self) -> Vec<AgentEntry> {
        let mut agents: Vec<AgentEntry> = self.lock().agents.values().cloned().collect();
        agents.sort_by(|left, right| {
            left.state
                .urgency()
                .cmp(&right.state.urgency())
                .then_with(|| left.state_since.cmp(&right.state_since))
        });
        agents
    }

    pub fn is_empty(&self) -> bool {
        let inner = self.lock();
        inner.agents.is_empty() && inner.subscribers.is_empty()
    }

    fn broadcast(inner: &mut Inner, event: Event) {
        // A subscriber whose channel is gone has disconnected; its connection
        // cleanup will run, but dropping it here keeps the map from growing.
        inner
            .subscribers
            .retain(|_, events| events.send(event.clone()).is_ok());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding this lock would be a bug in the daemon, not a
        // condition to recover from; taking the poisoned guard keeps the
        // remaining agents visible rather than killing every connection.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
