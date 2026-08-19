//! Who is connected, and who wants to be told about it.
//!
//! An entry exists exactly as long as the wrapper connection that registered
//! it. There is no persistence and no expiry: a wrapper that dies takes its
//! entry with it, which is what makes the registry self-healing after a crash
//! or a `kill -9` (ADR-0002).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use amon_protocol::{AgentEntry, AgentPatch, Event, SoundConfig};

/// Identifies one connection. Registry entries and subscriptions are keyed by
/// this so that dropping a connection cleans up everything it owned.
pub type ConnectionId = u64;

#[derive(Default)]
struct Inner {
    agents: HashMap<ConnectionId, AgentEntry>,
    subscribers: HashMap<ConnectionId, std::sync::mpsc::Sender<Event>>,
    /// Kept here so a transition can decide whether to make a noise without
    /// reaching back out to whatever read the file.
    sound: SoundConfig,
    sounds: crate::sound::Notifier,
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
        // Kept before the patch lands, because what is worth a noise is the
        // *crossing* — an agent already blocked that reports blocked again has
        // not started needing anyone.
        let was = (entry.state, entry.seen);
        if !patch.apply(entry) {
            return true;
        }
        let crossing = crate::sound::crossing(was, (entry.state, entry.seen));
        let agent = entry.id.clone();
        let event = Event::AgentUpdated(entry.clone());
        Self::broadcast(&mut inner, event);
        let sound_config = inner.sound.clone();
        let sounds = inner.sounds.clone();
        // Released first: what follows waits a second before it makes any
        // noise, and it must not do that holding the registry's lock.
        drop(inner);
        // Recorded even when it is worth no sound, because that still cancels
        // whatever the agent had pending.
        sounds.record(&agent, crossing, &sound_config);
        true
    }

    /// Hands every subscriber the configuration the daemon has just read.
    ///
    /// On the registry because the subscriber list is, and because a config
    /// change and an agent change are the same kind of thing to a bar: a push
    /// it did not ask for, applied on arrival.
    pub fn broadcast_config(&self, config: amon_protocol::Config) {
        let mut inner = self.lock();
        inner.sound = config.sound.clone();
        Self::broadcast(&mut inner, Event::ConfigChanged(config));
    }

    /// Forgets everything a connection owned. Called for every disconnect,
    /// whatever the reason.
    pub fn disconnect(&self, connection: ConnectionId) {
        let mut inner = self.lock();
        inner.subscribers.remove(&connection);
        if let Some(entry) = inner.agents.remove(&connection) {
            inner.sounds.forget(&entry.id);
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
            left.attention()
                .cmp(&right.attention())
                .then_with(|| left.state_since.cmp(&right.state_since))
        });
        agents
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
