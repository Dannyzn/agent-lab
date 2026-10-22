use agent_protocol::{BoxError, ModelClient, ModelResponse, ToolCall};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_INSTRUCTIONS: &str = "You are a coding agent. Work inside the provided workspace. Use tools to inspect, edit, and verify the project. Prefer the smallest correct change. When the task is complete, return a concise final message.";

#[derive(Clone)]
pub struct OpenAIResponsesClient {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    instructions: String,
}

impl OpenAIResponsesClient {
    pub fn new(
        model: impl Into<String>,
        api_key: impl Into<String>,
        base_url: impl AsRef<str>,
    ) -> Result<Self, BoxError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(anyhow!("OPENAI_API_KEY is required").into());
        }
        Ok(Self {
            client: reqwest::Client::builder().build()?,
            url: format!("{}/responses", base_url.as_ref().trim_end_matches('/')),
            api_key,
            model: model.into(),
            instructions: DEFAULT_INSTRUCTIONS.into(),
        })
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = instructions.into();
        self
    }

    async fn complete_once(
        &self,
        input_history: &[Value],
        tool_specs: &[Value],
    ) -> Result<ModelResponse, RequestError> {
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&json!({
                "model":self.model,
                "instructions":self.instructions,
                "input":input_history,
                "tools":tool_specs,
                "stream":true
            }))
            .send()
            .await
            .map_err(|error| RequestError::retryable(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let message = format!("Responses API returned {status}: {body}");
            return Err(RequestError {
                retryable: status.as_u16() == 429 || status.is_server_error(),
                message,
            });
        }

        let mut parsed = ModelResponse::default();
        let mut buffer = String::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| RequestError::retryable(error.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            if buffer.contains("\r\n") {
                buffer = buffer.replace("\r\n", "\n");
            }
            while let Some(boundary) = buffer.find("\n\n") {
                let frame = buffer[..boundary].to_owned();
                buffer.drain(..boundary + 2);
                parse_sse_frame(&frame, &mut parsed)
                    .map_err(|error| RequestError::retryable(error.to_string()))?;
            }
        }
        if !buffer.trim().is_empty() {
            parse_sse_frame(&buffer, &mut parsed)
                .map_err(|error| RequestError::retryable(error.to_string()))?;
        }
        Ok(parsed)
    }
}

#[async_trait]
impl ModelClient for OpenAIResponsesClient {
    async fn complete(
        &self,
        input_history: &[Value],
        tool_specs: &[Value],
    ) -> Result<ModelResponse, BoxError> {
        const ATTEMPTS: usize = 3;
        for attempt in 0..ATTEMPTS {
            match self.complete_once(input_history, tool_specs).await {
                Ok(response) => return Ok(response),
                Err(error) if error.retryable && attempt + 1 < ATTEMPTS => {
                    tokio::time::sleep(Duration::from_millis(200 * (1 << attempt))).await;
                }
                Err(error) => return Err(anyhow!(error.message).into()),
            }
        }
        unreachable!("retry loop always returns")
    }
}

struct RequestError {
    retryable: bool,
    message: String,
}

impl RequestError {
    fn retryable(message: String) -> Self {
        Self {
            retryable: true,
            message,
        }
    }
}

fn parse_sse_frame(frame: &str, response: &mut ModelResponse) -> Result<(), BoxError> {
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }
    let event: Value = serde_json::from_str(&data)
        .with_context(|| format!("invalid Responses SSE JSON: {data}"))?;
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_item.done") => {
            let item = &event["item"];
            match item.get("type").and_then(Value::as_str) {
                Some("function_call") => response.tool_calls.push(ToolCall {
                    call_id: required_string(item, "call_id")?.to_owned(),
                    name: required_string(item, "name")?.to_owned(),
                    arguments: serde_json::from_str(
                        item.get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}"),
                    )?,
                }),
                Some("message") => {
                    let text = item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|content| {
                            content.get("type").and_then(Value::as_str) == Some("output_text")
                        })
                        .filter_map(|content| content.get("text").and_then(Value::as_str))
                        .collect::<String>();
                    if !text.is_empty() {
                        response.text = Some(text);
                    }
                }
                _ => {}
            }
        }
        Some("response.completed") => {
            response.response_id = event
                .get("response")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        Some("error") => {
            return Err(anyhow!("Responses stream error: {event}").into());
        }
        _ => {}
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, name: &str) -> Result<&'a str, BoxError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("response item is missing {name}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_function_and_message_items() {
        let mut response = ModelResponse::default();
        parse_sse_frame(
            r#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"c1","name":"read_file","arguments":"{\"path\":\"README.md\"}"}}"#,
            &mut response,
        )
        .unwrap();
        parse_sse_frame(
            r#"data: {"type":"response.output_item.done","item":{"type":"message","content":[{"type":"output_text","text":"Done."}]}}"#,
            &mut response,
        )
        .unwrap();
        assert_eq!(response.tool_calls[0].name, "read_file");
        assert_eq!(response.text.as_deref(), Some("Done."));
    }
}
