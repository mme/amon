use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::AgentEntry;
use crate::config::Config;

/// Pushed by the daemon to every subscriber, unsolicited.
///
/// [`Event::Unknown`] is what makes a subscriber built against an older amon
/// survive a newer daemon: events it has never heard of land here and are
/// skipped rather than breaking the stream.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "event", content = "params")]
pub enum Event {
    /// An agent registered.
    #[serde(rename = "agent_connected")]
    AgentConnected(AgentEntry),
    /// An agent's entry changed — state, window, or session identity.
    #[serde(rename = "agent_updated")]
    AgentUpdated(AgentEntry),
    /// An agent's wrapper disconnected; its entry is gone.
    #[serde(rename = "agent_disconnected")]
    AgentDisconnected { id: String },
    /// The config file changed and has been re-read (ADR-0012). Carries the
    /// whole configuration rather than a diff: it is small, it is read rarely,
    /// and a subscriber that applies a whole answer cannot drift from one that
    /// missed an earlier event.
    #[serde(rename = "config_changed")]
    ConfigChanged(Config),
    /// A release newer than the running daemon exists. Broadcast when a
    /// check finds one, and replayed to every later subscriber — a panel
    /// restarted after the check must not wait a day to hear about it.
    /// Versions are bare (`0.2.0`), never tagged (`v0.2.0`).
    #[serde(rename = "update_available")]
    UpdateAvailable { installed: String, latest: String },
    /// An event this build does not know. Never sent, only received.
    #[serde(skip)]
    #[schemars(skip)]
    Unknown,
}

/// Hand-written so that an unrecognized event falls back to
/// [`Event::Unknown`] whatever its payload looks like. `#[serde(other)]`
/// cannot do this: it only tolerates an unknown tag when the content is
/// absent, and every future event will carry params.
impl<'de> Deserialize<'de> for Event {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let value = serde_json::Value::deserialize(deserializer)?;
        let name = value.get("event").and_then(serde_json::Value::as_str);
        let params = || {
            value
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        };

        Ok(match name {
            Some("agent_connected") => {
                Self::AgentConnected(serde_json::from_value(params()).map_err(D::Error::custom)?)
            }
            Some("agent_updated") => {
                Self::AgentUpdated(serde_json::from_value(params()).map_err(D::Error::custom)?)
            }
            Some("agent_disconnected") => {
                #[derive(Deserialize)]
                struct Params {
                    id: String,
                }
                let Params { id } = serde_json::from_value(params()).map_err(D::Error::custom)?;
                Self::AgentDisconnected { id }
            }
            Some("update_available") => {
                #[derive(Deserialize)]
                struct Params {
                    installed: String,
                    latest: String,
                }
                let Params { installed, latest } =
                    serde_json::from_value(params()).map_err(D::Error::custom)?;
                Self::UpdateAvailable { installed, latest }
            }
            _ => Self::Unknown,
        })
    }
}
