//! herdr's agent records, as the projection reads them.

use crate::runtime::HostedAgent;

/// The slice of herdr's agent record the projection needs. Everything else
/// on the wire is ignored, which is what keeps this compatible across herdr
/// releases.
///
/// Only what herdr's schema marks required is required here. `agent` and
/// `cwd` are nullable *and* optional there, and `Option` is what
/// accepts both spellings — a bare `String` with `#[serde(default)]` takes a
/// missing field but fails on an explicit `null`, and a failed record is
/// dropped by [`agents_from`], which is how a live agent would quietly
/// vanish from the bar.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct HerdrAgent {
    pub terminal_id: String,
    pub agent_status: String,
    pub pane_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// herdr's per-agent transition counter. It counts up through one
    /// agent's life, so a value that went *backwards* is a different agent
    /// in the same terminal — which is the only way to tell one claude from
    /// the next one started in the pane it just left.
    #[serde(default)]
    pub state_change_seq: Option<u64>,
}

impl From<HerdrAgent> for HostedAgent {
    fn from(agent: HerdrAgent) -> Self {
        HostedAgent {
            identity: agent.terminal_id,
            pane: agent.pane_id,
            agent: agent.agent,
            status: agent.agent_status,
            cwd: agent.cwd,
            life: agent.state_change_seq,
        }
    }
}

/// The agents out of a `session.snapshot` result. herdr 0.8.0 nests the
/// payload under `snapshot`; the docs draw it flat — read both, prefer what
/// the live server actually sends. Records missing a required field are
/// herdr's future, not our problem — skipped, never guessed at.
pub fn agents_from(result: &serde_json::Value) -> Vec<HerdrAgent> {
    result
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("agents"))
        .or_else(|| result.get("agents"))
        .and_then(|agents| agents.as_array())
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| serde_json::from_value(agent.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_herdr_agent_becomes_a_hosted_agent_keyed_by_terminal() {
        let hosted: HostedAgent = HerdrAgent {
            terminal_id: "t1".into(),
            agent_status: "done".into(),
            pane_id: "w1:p2".into(),
            agent: Some("claude".into()),
            cwd: None,
            state_change_seq: Some(3),
        }
        .into();
        assert_eq!(hosted.identity, "t1");
        assert_eq!(hosted.pane, "w1:p2");
        assert_eq!(hosted.status, "done");
        assert_eq!(hosted.life, Some(3));
    }

    #[test]
    fn snapshot_agents_are_extracted_and_partial_records_skipped() {
        // The shape herdr 0.8.0 actually sends: the payload nested under
        // "snapshot" (verified against a live server).
        let snapshot: serde_json::Value = serde_json::json!({
            "type": "session_snapshot",
            "snapshot": {
                "version": "0.8.0",
                "agents": [
                    {"terminal_id":"t1","agent":"claude","agent_status":"working",
                     "pane_id":"w1:p1","cwd":"/repo"},
                    {"agent":"mystery"}
                ]
            }
        });
        let agents = agents_from(&snapshot);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].terminal_id, "t1");
        assert_eq!(agents[0].agent_status, "working");
        assert_eq!(agents[0].pane_id, "w1:p1");
    }

    #[test]
    fn an_agent_survives_the_fields_herdr_is_allowed_to_leave_out() {
        // herdr's schema requires only terminal_id, pane_id and
        // agent_status; `agent` and `cwd` are nullable and
        // optional. A null there must not delete the agent from amon —
        // silently dropping the record is how a live agent vanishes from
        // the bar for a reason nobody can see.
        let snapshot = serde_json::json!({
            "agents": [
                {"terminal_id":"t1","pane_id":"w1:p1","agent_status":"blocked",
                 "agent": null, "cwd": null},
                {"terminal_id":"t2","pane_id":"w1:p2","agent_status":"idle"},
            ]
        });
        let agents = agents_from(&snapshot);
        assert_eq!(agents.len(), 2, "nullable fields are not required fields");
        assert_eq!(agents[0].agent, None);
        assert_eq!(agents[0].cwd, None);
        assert_eq!(agents[1].terminal_id, "t2");
    }

    #[test]
    fn a_record_missing_what_herdr_always_sends_is_still_skipped() {
        let snapshot = serde_json::json!({"agents": [{"agent": "mystery"}]});
        assert!(agents_from(&snapshot).is_empty());
    }

    #[test]
    fn a_snapshot_with_agents_at_the_top_level_reads_the_same() {
        // The 0.8.2 docs draw the response flat; accept both spellings so a
        // future herdr un-nesting it costs nothing.
        let snapshot: serde_json::Value = serde_json::json!({
            "type": "session_snapshot",
            "agents": [
                {"terminal_id":"t1","agent":"claude","agent_status":"idle",
                 "pane_id":"w1:p1"}
            ]
        });
        assert_eq!(agents_from(&snapshot).len(), 1);
    }
}
