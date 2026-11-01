#![allow(clippy::double_must_use)]

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::error::Error;

pub type BoxError = Box<dyn Error + Send + Sync>;

/// Local payload estimates are conservative bytes, not provider billing tokens.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContextPolicy {
    pub max_tokens: usize,
    pub reserve_output_tokens: usize,
    pub tool_output_bytes: usize,
    pub keep_recent_pairs: usize,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            max_tokens: 32768,
            reserve_output_tokens: 4096,
            tool_output_bytes: 8192,
            keep_recent_pairs: 2,
        }
    }
}

/// Sticky cancellation: late subscribers also observe a prior cancellation.
#[derive(Clone, Debug)]
pub struct CancellationToken(tokio::sync::watch::Sender<bool>);

impl Default for CancellationToken {
    fn default() -> Self {
        Self(tokio::sync::watch::channel(false).0)
    }
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub tool: String,
    pub arguments: Value,
    pub reason: String,
    pub capabilities: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn approve(&self, request: &ApprovalRequest) -> Result<ApprovalDecision, BoxError>;
}

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
