//! The agent layer: the one arrangement of the Creator Micro 2's keymap
//! under which its keys report to amon instead of typing.
//!
//! A layer whose keys are the firmware's vendor keycodes (`KV_OAI_*`) emits
//! events on the RPC channel and lets the host light the six agent keys;
//! any other layer types letters and lights the way the Input app said. The
//! board keeps up to six layers in flash and the touch sensor cycles them,
//! so this is a setup step, and one amon owns (ADR-0022): read the keymap,
//! put the layer on an empty slot, write it back. Everything here except the
//! two I/O calls is pure and tested against real boards' keymaps.

/// The layout the firmware's own Codex profile uses, and the one amon
/// speaks: six agent keys across the top two rows, seven action keys below,
/// the encoder's two directions and click, the joystick as a vendor
/// device. Copied from a board that had it written by Work Louder's app,
/// so it is their arrangement, not a guess at it.
pub const AGENT_LAYOUT: &str = r#"{
  "keymap": [
    ["KV_OAI_AG00", "KV_OAI_AG01"],
    ["KV_OAI_AG02", "KV_OAI_AG03", "KV_OAI_AG04", "KV_OAI_AG05"],
    ["KV_OAI_ACT06", "KV_OAI_ACT07", "KV_OAI_ACT08", "KV_OAI_ACT09"],
    ["KV_OAI_ACT10", "KV_OAI_ACT11", "KV_OAI_ACT12"]
  ],
  "encoders": [["KV_OAI_ENC_CC", "KV_OAI_ENC_CW", "KV_OAI_ENC_CLK"]],
  "joystick": {"type": "VENDOR", "sectors": []}
}"#;

/// The first firmware that speaks the `v.oai.*` methods.
pub const FIRMWARE_MIN: (u32, u32, u32) = (0, 6, 0);

/// The name the layer gets, so it is recognisable in Work Louder's app.
const LAYER_NAME: &str = "amon";

/// Where the board's active profile keeps its layers.
fn layers(keymap: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    let active = keymap
        .get("activeProfileId")
        .and_then(serde_json::Value::as_u64);
    let profiles = keymap.get("profiles")?.as_array()?;
    let profile = match active {
        Some(id) => profiles
            .iter()
            .find(|p| p.get("id").and_then(serde_json::Value::as_u64) == Some(id))
            .or_else(|| profiles.first()),
        None => profiles.first(),
    }?;
    profile.get("layers")?.as_array()
}

fn layers_mut(keymap: &mut serde_json::Value) -> Option<&mut Vec<serde_json::Value>> {
    let active = keymap
        .get("activeProfileId")
        .and_then(serde_json::Value::as_u64);
    let profiles = keymap.get_mut("profiles")?.as_array_mut()?;
    let position = match active {
        Some(id) => profiles
            .iter()
            .position(|p| p.get("id").and_then(serde_json::Value::as_u64) == Some(id))
            .unwrap_or(0),
        None => 0,
    };
    profiles
        .get_mut(position)?
        .get_mut("layers")?
        .as_array_mut()
}

fn keycodes(layer: &serde_json::Value) -> Vec<&str> {
    layer
        .pointer("/layout/keymap")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .flat_map(|row| row.as_array().into_iter().flatten())
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default()
}

/// How many layers the active profile has.
pub fn layer_count(keymap: &serde_json::Value) -> usize {
    layers(keymap).map(Vec::len).unwrap_or(0)
}

/// The 0-based index of the layer whose keys report to the host, if any.
/// One agent key is proof enough: the firmware's own profile never mixes.
pub fn agent_layer(keymap: &serde_json::Value) -> Option<usize> {
    layers(keymap)?
        .iter()
        .position(|layer| keycodes(layer).contains(&"KV_OAI_AG00"))
}

/// The 0-based index of the first layer that does nothing at all — every
/// key `KC_NONE` — which is where the agent layer can go without taking
/// anything from anyone. A fresh board ships with two.
pub fn empty_layer(keymap: &serde_json::Value) -> Option<usize> {
    layers(keymap)?.iter().position(|layer| {
        let keys = keycodes(layer);
        !keys.is_empty() && keys.iter().all(|key| *key == "KC_NONE")
    })
}

/// The board holds six layers at most; the Input app enforces the same.
const MAX_LAYERS: usize = 6;

/// Where the agent layer would be written: an empty slot, or a new one when
/// there is room. `None` when the board is full of layers that all do
/// something — then it is the user's call which to give up, in the app.
pub fn slot_for_agent_layer(keymap: &serde_json::Value) -> Option<usize> {
    if let Some(index) = empty_layer(keymap) {
        return Some(index);
    }
    let count = layer_count(keymap);
    (count < MAX_LAYERS).then_some(count)
}

/// The keymap with the agent layer at `index`, everything else untouched.
/// An existing layer keeps its id, colour and lights; only its layout and
/// name change. A new layer is appended in the same shape as the others.
pub fn with_agent_layer(keymap: &serde_json::Value, index: usize) -> serde_json::Value {
    let mut out = keymap.clone();
    let layout: serde_json::Value =
        serde_json::from_str(AGENT_LAYOUT).expect("static layout is JSON");
    let Some(layers) = layers_mut(&mut out) else {
        return out;
    };
    if index < layers.len() {
        layers[index]["layout"] = layout;
        layers[index]["name"] = serde_json::Value::String(LAYER_NAME.into());
    } else {
        layers.push(serde_json::json!({
            "id": index,
            "name": LAYER_NAME,
            "color": 16711680,
            "layout": layout,
        }));
    }
    out
}

/// Whether a firmware version string is new enough for the agent keys.
/// Versions look like `0.6.3-rc.10`; a release candidate of 0.6 counts,
/// since that is what shipped the protocol.
pub fn firmware_supports(version: &str) -> bool {
    let core = version.trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut parts = core.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    let found = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    found >= FIRMWARE_MIN
}

/// What doctor says about the layer situation, given the keymap and the
/// board's 1-based active layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerReport {
    /// The agent layer exists and is the one the board is on.
    Active { layer: usize, total: usize },
    /// It exists, but the board is on another layer.
    Inactive {
        layer: usize,
        active: usize,
        total: usize,
    },
    /// No layer on the board reports to the host.
    Missing { slot: Option<usize>, total: usize },
}

pub fn report(keymap: &serde_json::Value, active_1based: usize) -> LayerReport {
    let total = layer_count(keymap);
    match agent_layer(keymap) {
        Some(index) => {
            let layer = index + 1;
            if layer == active_1based {
                LayerReport::Active { layer, total }
            } else {
                LayerReport::Inactive {
                    layer,
                    active: active_1based,
                    total,
                }
            }
        }
        None => LayerReport::Missing {
            slot: slot_for_agent_layer(keymap).map(|index| index + 1),
            total,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A board Work Louder's app wrote the Codex layer onto (layer 1), read
    /// back over fs.read on 2026-09-30, firmware 0.6.3-rc.10.
    const WITH_AGENT_LAYER: &str = r#"{"version":1,"activeProfileId":0,"profiles":[{"id":0,"name":"Default","layers":[{"id":0,"name":"Layer 1","color":16711680,"layout":{"keymap":[["KV_OAI_AG00","KV_OAI_AG01"],["KV_OAI_AG02","KV_OAI_AG03","KV_OAI_AG04","KV_OAI_AG05"],["KV_OAI_ACT06","KV_OAI_ACT07","KV_OAI_ACT08","KV_OAI_ACT09"],["KV_OAI_ACT10","KV_OAI_ACT11","KV_OAI_ACT12"]],"encoders":[["KV_OAI_ENC_CC","KV_OAI_ENC_CW","KV_OAI_ENC_CLK"]],"joystick":{"type":"VENDOR","sectors":[]}},"lights":{"backlight":{"effect":"solid","brightness":1,"speed":0.5,"magic":1,"color":16777215},"underglow":{"effect":"rainbow","brightness":1,"speed":0.55,"magic":1,"color":16777215}}},{"id":1,"name":"Layer 2","color":16711680,"layout":{"keymap":[["KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE"]],"encoders":[["KC_NONE","KC_NONE","KC_NONE"]],"joystick":{"type":"JOYSTICK","sectors":[]}},"lights":{"backlight":{"effect":"solid","brightness":1,"speed":0.5,"magic":1,"color":16777215},"underglow":{"effect":"rainbow","brightness":1,"speed":0.55,"magic":1,"color":16777215}}},{"id":2,"name":"Layer 3","color":16711680,"layout":{"keymap":[["KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE"]],"encoders":[["KC_NONE","KC_NONE","KC_NONE"]],"joystick":{"type":"JOYSTICK","sectors":[]}}}],"macrosUsed":[],"multiActionsUsed":[]}],"multiActions":[],"macros":[],"macrosGroups":[],"multiActionsGroups":[],"linkedApps":[]}"#;

    /// The same board fresh from the factory: letters on layer 1, two empty
    /// layers behind it. Backed up before anything was written, 2026-07-31.
    const FACTORY: &str = r#"{"version":1,"activeProfileId":0,"profiles":[{"id":0,"name":"Default","layers":[{"id":0,"name":"Layer 1","color":16711680,"layout":{"keymap":[["KC_A","KC_B"],["KC_C","KC_D","KC_E","KC_F"],["KC_G","KC_H","KC_I","KC_J"],["KC_K","KC_L","KC_M"]],"encoders":[["KC_VOLU","KC_VOLD","KC_MPLY"]],"joystick":{"type":"RADIAL","sectors":[{"k":"KI_X","a1":0.1875,"a2":0.3125}]}},"lights":{"backlight":{"effect":"solid","brightness":1,"speed":0,"magic":1,"color":16777215}}},{"id":1,"name":"Layer 2","color":16711680,"layout":{"keymap":[["KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE"]],"encoders":[["KC_NONE","KC_NONE","KC_NONE"]],"joystick":{"type":"JOYSTICK","sectors":[]}},"lights":{"backlight":{"effect":"solid","brightness":1,"speed":0.5,"magic":1,"color":16777215}}},{"id":2,"name":"Layer 3","color":16711680,"layout":{"keymap":[["KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE","KC_NONE"],["KC_NONE","KC_NONE","KC_NONE"]],"encoders":[["KC_NONE","KC_NONE","KC_NONE"]],"joystick":{"type":"JOYSTICK","sectors":[]}}}],"macrosUsed":[],"multiActionsUsed":[]}],"multiActions":[],"macros":[],"macrosGroups":[],"multiActionsGroups":[],"linkedApps":[]}"#;

    fn parse(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_board_with_the_layer_written_is_found_on_its_first_layer() {
        let keymap = parse(WITH_AGENT_LAYER);
        assert_eq!(agent_layer(&keymap), Some(0));
        assert_eq!(layer_count(&keymap), 3);
        // The board reports layer 1 (1-based) while the keys work, which is
        // how the base of device.status was settled.
        assert_eq!(
            report(&keymap, 1),
            LayerReport::Active { layer: 1, total: 3 }
        );
        assert_eq!(
            report(&keymap, 2),
            LayerReport::Inactive {
                layer: 1,
                active: 2,
                total: 3
            }
        );
    }

    #[test]
    fn a_factory_board_has_no_agent_layer_and_two_empty_slots() {
        let keymap = parse(FACTORY);
        assert_eq!(agent_layer(&keymap), None);
        assert_eq!(empty_layer(&keymap), Some(1));
        assert_eq!(slot_for_agent_layer(&keymap), Some(1));
        assert_eq!(
            report(&keymap, 1),
            LayerReport::Missing {
                slot: Some(2),
                total: 3
            }
        );
    }

    #[test]
    fn writing_the_layer_touches_one_slot_and_nothing_else() {
        let keymap = parse(FACTORY);
        let written = with_agent_layer(&keymap, 1);
        assert_eq!(agent_layer(&written), Some(1));
        // The letters layer is untouched, byte for byte.
        assert_eq!(
            written.pointer("/profiles/0/layers/0"),
            keymap.pointer("/profiles/0/layers/0")
        );
        // The slot kept its id and lights, and took the name.
        assert_eq!(
            written.pointer("/profiles/0/layers/1/id"),
            keymap.pointer("/profiles/0/layers/1/id")
        );
        assert_eq!(
            written.pointer("/profiles/0/layers/1/lights"),
            keymap.pointer("/profiles/0/layers/1/lights")
        );
        assert_eq!(
            written
                .pointer("/profiles/0/layers/1/name")
                .and_then(|v| v.as_str()),
            Some("amon")
        );
        // Writing what a board already has changes nothing.
        let already = parse(WITH_AGENT_LAYER);
        assert_eq!(
            with_agent_layer(&already, 0).pointer("/profiles/0/layers/0/layout"),
            already.pointer("/profiles/0/layers/0/layout")
        );
    }

    #[test]
    fn a_board_with_no_empty_layer_gets_one_appended_until_six() {
        let mut keymap = parse(FACTORY);
        // Fill the empty slots with something.
        for index in 1..3 {
            keymap["profiles"][0]["layers"][index]["layout"]["keymap"][0][0] = "KC_A".into();
        }
        assert_eq!(empty_layer(&keymap), None);
        assert_eq!(slot_for_agent_layer(&keymap), Some(3));
        let written = with_agent_layer(&keymap, 3);
        assert_eq!(layer_count(&written), 4);
        assert_eq!(agent_layer(&written), Some(3));
        assert_eq!(
            written
                .pointer("/profiles/0/layers/3/id")
                .and_then(|v| v.as_u64()),
            Some(3)
        );
        // Six layers that all do something: nowhere to go.
        let mut full = written;
        while layer_count(&full) < 6 {
            let n = layer_count(&full);
            full["profiles"][0]["layers"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "id": n, "name": format!("Layer {}", n + 1), "color": 0,
                    "layout": {"keymap": [["KC_A"]], "encoders": [], "joystick": {}}
                }));
        }
        full["profiles"][0]["layers"][3]["layout"]["keymap"][0][0] = "KC_B".into();
        assert_eq!(slot_for_agent_layer(&full), None);
    }

    #[test]
    fn firmware_before_0_6_cannot_do_this() {
        assert!(!firmware_supports("0.4.0"));
        assert!(!firmware_supports("v0.5.9"));
        assert!(firmware_supports("0.6.0"));
        assert!(firmware_supports("0.6.0-rc.13"));
        assert!(firmware_supports("0.6.3-rc.10"));
        assert!(firmware_supports("v0.6.2"));
        assert!(firmware_supports("1.0.0"));
        assert!(!firmware_supports("garbage"));
    }
}
