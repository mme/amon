//! Keeps the checked-in JSON Schema honest.
//!
//! Non-Rust peers (the shell and JavaScript integration hooks) validate against
//! `docs/protocol.schema.json`, so it has to track the Rust types. Run with
//! `UPDATE_SCHEMA=1` to rewrite it after an intentional protocol change.

use std::path::PathBuf;

fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docs/protocol.schema.json")
}

#[test]
fn checked_in_schema_matches_the_types() {
    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(&gaze_protocol::protocol_schema()).unwrap()
    );
    let path = schema_path();

    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
        return;
    }

    let checked_in = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        checked_in, generated,
        "docs/protocol.schema.json is stale; rerun with UPDATE_SCHEMA=1"
    );
}
