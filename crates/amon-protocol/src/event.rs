use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::AgentEntry;

/// Pushed by the daemon to every subscriber, unsolicited.
///
/// [`Event::Unknown`] is what makes a subscriber built against an older amon
/// survive a newer daemon: events it has never heard of land here and are
/// skipped rather than breaking the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "event", content = "params")]
pub enum Event {
    /// An agent registered.
    #[serde(rename = "agent_connected")]
    AgentConnected(AgentEntry),
    /// An agent's entry changed — state, title, or session identity.
    #[serde(rename = "agent_updated")]
    AgentUpdated(AgentEntry),
    /// An agent's wrapper disconnected; its entry is gone.
    #[serde(rename = "agent_disconnected")]
    AgentDisconnected { id: String },
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
            _ => Self::Unknown,
        })
    }
}
