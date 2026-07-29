use schemars::JsonSchema;

use crate::event::Event;
use crate::frame::{Request, Response};

/// The whole protocol in one document, so that non-Rust peers — the installed
/// integration hooks are shell and JavaScript — have something to validate
/// against.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct Protocol {
    request: Request,
    response: Response,
    event: Event,
}

/// The generated JSON Schema. `docs/protocol.schema.json` is this, checked in;
/// a test fails when the two drift.
pub fn protocol_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Protocol)).expect("schema serializes")
}
