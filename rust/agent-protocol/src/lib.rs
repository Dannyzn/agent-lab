#![allow(clippy::double_must_use)]

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::error::Error;

pub type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ModelResponse {
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn complete(
        &self,
        input_history: &[Value],
        tool_specs: &[Value],
    ) -> Result<ModelResponse, BoxError>;
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &Value) -> Result<(), BoxError>;
}

pub fn event(event_type: &str, payload: Value) -> Value {
    let mut fields = match payload {
        Value::Object(fields) => fields,
        _ => Map::new(),
    };
    fields.insert("type".into(), Value::String(event_type.into()));
    fields.insert("timestamp".into(), Value::String(Utc::now().to_rfc3339()));
    Value::Object(fields)
}
