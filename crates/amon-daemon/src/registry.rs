//! Who is connected, and who wants to be told about it.
//!
//! An entry exists exactly as long as the wrapper connection that registered
//! it. There is no persistence and no expiry: a wrapper that dies takes its
//! entry with it, which is what makes the registry self-healing after a crash
//! or a `kill -9` (ADR-0002).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use amon_protocol::{AgentEntry, AgentPatch, Event, SoundConfig};

use crate::sound::Sound;

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
    /// The newer release the last check found, as (installed, latest).
    /// Held so a subscriber arriving after the check hears about it too.
    update: Option<(String, String)>,
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
    /// after reconnecting, and the herdr projection re-registers after every
    /// resnapshot, so seeing the same id twice is normal.
    ///
    /// Replacing an entry is an update, and it goes through the same sound
    /// arbitration [`Registry::update`] does — otherwise a state learned this
    /// way is both silent and unable to cancel what an earlier crossing had
    /// pending, so an agent that stopped being blocked still rings.
    pub fn register(&self, connection: ConnectionId, entry: AgentEntry) {
        let mut inner = self.lock();
        let existing = inner.agents.insert(connection, entry.clone());
        // The agent as the registry last knew it, from this connection or
        // from one still handing it over. Pending noises are tracked per
        // agent id, so the crossing has to be measured the same way, or a
        // replacement claiming an agent mid-handover neither rings for what
        // it found nor cancels what the old claim left pending.
        let superseded: Vec<ConnectionId> = inner
            .agents
            .iter()
            .filter(|(held, other)| **held != connection && other.id == entry.id)
            .map(|(held, _)| *held)
            .collect();
        let previous = existing
            .clone()
            .filter(|previous| previous.id == entry.id)
            .or_else(|| {
                superseded
                    .first()
                    .and_then(|held| inner.agents.get(held).cloned())
            });
        // One row per public id. A replacement claiming an agent supersedes
        // the claim it replaces rather than standing beside it until the old
        // one lets go: two entries for one agent are two rows in `status`,
        // and a panel seeded in that moment can keep the stale one.
        for held in superseded {
            inner.agents.remove(&held);
        }
        let crossing = upsert_crossing(previous.as_ref(), &entry);
        let agent = entry.id.clone();
        let event = match existing {
            Some(previous) if previous.id == entry.id => Event::AgentUpdated(entry),
            _ => Event::AgentConnected(entry),
        };
        Self::broadcast(&mut inner, event);
        let Some(crossing) = crossing else {
            return;
        };
        let sound_config = inner.sound.clone();
        let sounds = inner.sounds.clone();
        // Released first, for the same reason `update` releases it: what
        // follows waits a second before it makes any noise.
        drop(inner);
        sounds.record(&agent, crossing, &sound_config);
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
    ///
    /// An agent another connection still holds has not disconnected, only
    /// changed hands: when a herdr session's client is replaced, the new
    /// projection claims the same public ids before the old one has finished
    /// letting go. Subscribers forget by id, so announcing that would take a
    /// live row off every bar — and nothing is guaranteed to put it back,
    /// because an idle agent may never change state again.
    pub fn disconnect(&self, connection: ConnectionId) {
        let mut inner = self.lock();
        inner.subscribers.remove(&connection);
        let Some(entry) = inner.agents.remove(&connection) else {
            return;
        };
        if inner.agents.values().any(|held| held.id == entry.id) {
            return;
        }
        inner.sounds.forget(&entry.id);
        Self::broadcast(&mut inner, Event::AgentDisconnected { id: entry.id });
    }

    pub fn subscribe(&self, connection: ConnectionId, events: std::sync::mpsc::Sender<Event>) {
        let mut inner = self.lock();
        // A known update is replayed rather than waited for: checks are at
        // most daily, and the subscriber asking is usually a panel that was
        // just restarted — the same person the original broadcast missed.
        if let Some((installed, latest)) = inner.update.clone() {
            let _ = events.send(Event::UpdateAvailable { installed, latest });
        }
        inner.subscribers.insert(connection, events);
    }

    /// Records that a newer release exists and tells every subscriber —
    /// once: the daily re-check finding the same release again is not news.
    pub fn publish_update(&self, installed: String, latest: String) {
        let mut inner = self.lock();
        let update = (installed, latest);
        if inner.update.as_ref() == Some(&update) {
            return;
        }
        inner.update = Some(update.clone());
        let (installed, latest) = update;
        Self::broadcast(&mut inner, Event::UpdateAvailable { installed, latest });
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

/// What replacing `previous` with `entry` is worth hearing, if it is a
/// replacement at all.
///
/// `None` means this connection is claiming an entry rather than revising
/// one, and a fresh arrival is not a crossing: an agent that is already
/// blocked when amon first sees it has not just become blocked. The inner
/// `None` is a revision that crossed nothing — still worth recording,
/// because recording is also what cancels a noise the previous state had
/// pending.
fn upsert_crossing(previous: Option<&AgentEntry>, entry: &AgentEntry) -> Option<Option<Sound>> {
    let previous = previous.filter(|previous| previous.id == entry.id)?;
    Some(crate::sound::crossing(
        (previous.state, previous.seen),
        (entry.state, entry.seen),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use amon_protocol::AgentState;

    fn entry(state: AgentState, seen: Option<bool>) -> AgentEntry {
        AgentEntry {
            id: "a1".into(),
            agent: "claude".into(),
            state,
            state_since: 0,
            cwd: "/".into(),
            pid: 1,
            args: Vec::new(),
            hostname: "h".into(),
            started_at: 0,
            title: None,
            agent_session_id: None,
            agent_session_path: None,
            window: None,
            workspace: None,
            branch: None,
            focused: None,
            seen,
            herdr: None,
        }
    }

    #[test]
    fn a_first_claim_is_not_a_crossing() {
        // Whatever an agent is doing when amon first sees it, it did not
        // just start doing it.
        assert_eq!(
            upsert_crossing(None, &entry(AgentState::Blocked, None)),
            None
        );
    }

    #[test]
    fn a_different_agent_on_the_same_connection_is_a_first_claim() {
        let mut previous = entry(AgentState::Working, None);
        previous.id = "gone".into();
        assert_eq!(
            upsert_crossing(Some(&previous), &entry(AgentState::Blocked, None)),
            None
        );
    }

    #[test]
    fn a_revision_carries_the_crossing_it_made() {
        // The case a resnapshot hits: the state moved while amon was not
        // listening to events, and it is still worth hearing.
        assert_eq!(
            upsert_crossing(
                Some(&entry(AgentState::Working, None)),
                &entry(AgentState::Blocked, None)
            ),
            Some(Some(Sound::Blocked))
        );
        assert_eq!(
            upsert_crossing(
                Some(&entry(AgentState::Working, None)),
                &entry(AgentState::Idle, Some(false))
            ),
            Some(Some(Sound::Done))
        );
    }

    #[test]
    fn a_handover_is_not_a_disconnect() {
        // Two clients share a herdr session; the one amon follows exits, and
        // the replacement's projection claims the same public ids from a new
        // connection before the old one has finished letting go. Subscribers
        // forget by id, so announcing that disconnect would take a live row
        // off every bar — and nothing is guaranteed to put it back, because
        // an idle agent may never change state again.
        let registry = Registry::new();
        let (events, inbox) = std::sync::mpsc::channel();
        registry.subscribe(1, events);

        registry.register(10, entry(AgentState::Working, None));
        registry.register(20, entry(AgentState::Working, None));
        // One row throughout: the replacement supersedes the claim it
        // replaces rather than standing beside it.
        assert_eq!(registry.agents().len(), 1);
        registry.disconnect(10);

        let seen: Vec<Event> = inbox.try_iter().collect();
        assert!(
            !seen
                .iter()
                .any(|event| matches!(event, Event::AgentDisconnected { .. })),
            "connection 20 still holds the agent: {seen:?}"
        );
        assert_eq!(registry.agents().len(), 1);

        // The last holder letting go is a real disconnect.
        registry.disconnect(20);
        let seen: Vec<Event> = inbox.try_iter().collect();
        assert!(
            seen.iter()
                .any(|event| matches!(event, Event::AgentDisconnected { .. })),
            "nothing holds the agent now: {seen:?}"
        );
        assert!(registry.agents().is_empty());
    }

    #[test]
    fn a_revision_that_crossed_nothing_still_records() {
        // Recording is what cancels: an agent that stops being blocked
        // before the settle delay elapses must not ring afterwards, and the
        // inner `None` is what silences it.
        assert_eq!(
            upsert_crossing(
                Some(&entry(AgentState::Blocked, None)),
                &entry(AgentState::Working, None)
            ),
            Some(None)
        );
    }
}
