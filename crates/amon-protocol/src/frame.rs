use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::event::Event;
use crate::method::{Method, MethodError};

/// One request frame: `{"id": …, "method": …, "params": …}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub method: Method,
}

/// The envelope, parsed before the method is. Keeping these two steps apart is
/// what lets us answer a request whose method we do not understand.
#[derive(Debug, Deserialize)]
struct Envelope {
    id: String,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// A frame we could not turn into a [`Request`]. `id` is present whenever the
/// envelope itself parsed, so the caller can still send an error response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub id: Option<String>,
    pub error: Error,
}

impl Request {
    pub fn new(id: impl Into<String>, method: Method) -> Self {
        Self {
            id: id.into(),
            method,
        }
    }

    pub fn parse(line: &str) -> Result<Self, ParseError> {
        let envelope: Envelope = serde_json::from_str(line).map_err(|source| ParseError {
            id: None,
            error: Error::new(ErrorCode::InvalidFrame, source.to_string()),
        })?;
        match Method::parse(&envelope.method, envelope.params) {
            Ok(method) => Ok(Self {
                id: envelope.id,
                method,
            }),
            Err(error) => Err(ParseError {
                id: Some(envelope.id),
                error: error.into(),
            }),
        }
    }

    /// Serializes to a single line, newline included.
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("request serializes");
        line.push('\n');
        line
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

impl Response {
    pub fn ok(id: impl Into<String>, result: &impl Serialize) -> Self {
        Self {
            id: id.into(),
            result: Some(serde_json::to_value(result).expect("result serializes")),
            error: None,
        }
    }

    /// A response with no payload, for methods that only need acknowledging.
    pub fn ack(id: impl Into<String>) -> Self {
        Self::ok(id, &serde_json::json!({}))
    }

    pub fn err(id: impl Into<String>, error: Error) -> Self {
        Self {
            id: id.into(),
            result: None,
            error: Some(error),
        }
    }

    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("response serializes");
        line.push('\n');
        line
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The frame was not a JSON object with an `id` and a `method`.
    InvalidFrame,
    /// A method this peer does not implement — including one from a newer amon.
    UnsupportedMethod,
    InvalidParams,
    /// No registry entry with that id.
    UnknownAgent,
    Internal,
}

impl From<MethodError> for Error {
    fn from(error: MethodError) -> Self {
        let code = match error {
            MethodError::Unsupported(_) => ErrorCode::UnsupportedMethod,
            MethodError::InvalidParams(_) => ErrorCode::InvalidParams,
        };
        Self::new(code, error.to_string())
    }
}

/// What a client reads from the daemon: either an answer to something it asked,
/// or an event it subscribed to.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerFrame {
    Response(Response),
    /// Boxed because an event carries a whole [`crate::AgentEntry`] and a
    /// response carries almost nothing, so an unboxed variant would make every
    /// response as large as the largest event.
    Event(Box<Event>),
}

impl ServerFrame {
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.get("event").is_some() {
            serde_json::from_value(value).map(Box::new).map(Self::Event)
        } else {
            serde_json::from_value(value).map(Self::Response)
        }
    }
}
