use agent_protocol::{BoxError, EventSink, ModelClient, ToolCall, event};
use agent_tools::ToolRegistry;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub struct Agent {
    model: Arc<dyn ModelClient>,
    tools: ToolRegistry,
    events: Arc<dyn EventSink>,
    max_steps: usize,
}

#[derive(Debug)]
pub struct RunResult {
    pub thread_id: String,
    pub turn_id: String,
    pub final_message: String,
}

impl Agent {
    pub fn new(
        model: Arc<dyn ModelClient>,
        tools: ToolRegistry,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            model,
            tools,
            events,
            max_steps: 16,
        }
    }

    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    pub async fn run(&self, prompt: &str) -> Result<RunResult> {
        let thread_id = Uuid::new_v4().to_string();
        let turn_id = Uuid::new_v4().to_string();
        self.emit("thread.started", json!({"thread_id":thread_id}))?;
        self.emit(
            "turn.started",
            json!({"thread_id":thread_id,"turn_id":turn_id}),
        )?;
        match self.run_turn(prompt, &thread_id, &turn_id).await {
            Ok(final_message) => {
                self.emit(
                    "turn.completed",
                    json!({"thread_id":thread_id,"turn_id":turn_id}),
                )?;
                Ok(RunResult {
                    thread_id,
                    turn_id,
                    final_message,
                })
            }
            Err(error) => {
                let failure = self.emit(
                    "turn.failed",
                    json!({
                        "thread_id":thread_id,
                        "turn_id":turn_id,
                        "error":{"type":"runtime_error","message":error.to_string()}
                    }),
                );
                if let Err(event_error) = failure {
                    return Err(anyhow!(
                        "{error}; additionally failed to record error: {event_error}"
                    ));
                }
                Err(error)
            }
        }
    }

    async fn run_turn(&self, prompt: &str, thread_id: &str, turn_id: &str) -> Result<String> {
        let mut history = vec![json!({
            "type":"message",
            "role":"user",
            "content":[{"type":"input_text","text":prompt}]
        })];
        let tool_specs = self.tools.specs();
        for _ in 0..self.max_steps {
            let response = self
                .model
                .complete(&history, &tool_specs)
                .await
                .map_err(boxed_error)?;
            if !response.tool_calls.is_empty() {
                for call in response.tool_calls {
                    history.push(json!({
                        "type":"function_call",
                        "call_id":call.call_id,
                        "name":call.name,
                        "arguments":serde_json::to_string(&call.arguments)?
                    }));
                    history.push(self.execute_call(thread_id, turn_id, &call).await?);
                }
                continue;
            }
            let text = response
                .text
                .ok_or_else(|| anyhow!("model returned neither tool calls nor final text"))?;
            self.emit(
                "item.completed",
                json!({
                    "thread_id":thread_id,
                    "turn_id":turn_id,
                    "item":{"id":Uuid::new_v4().to_string(),"type":"agent_message","text":text}
                }),
            )?;
            return Ok(text);
        }
        Err(anyhow!("agent exceeded max_steps={}", self.max_steps))
    }

    async fn execute_call(&self, thread_id: &str, turn_id: &str, call: &ToolCall) -> Result<Value> {
        let item_type = match call.name.as_str() {
            "exec_command" => "command_execution",
            "apply_patch" => "file_change",
            _ => "tool_call",
        };
        let item = json!({
            "id":call.call_id,
            "type":item_type,
            "name":call.name,
            "arguments":call.arguments,
            "status":"in_progress"
        });
        self.emit(
            "item.started",
            json!({"thread_id":thread_id,"turn_id":turn_id,"item":item}),
        )?;
        let result = self.tools.execute(&call.name, &call.arguments).await;
        self.emit(
            "item.completed",
            json!({
                "thread_id":thread_id,
                "turn_id":turn_id,
                "item":{
                    "id":call.call_id,
                    "type":item_type,
                    "name":call.name,
                    "arguments":call.arguments,
                    "status":if result.success {"completed"} else {"failed"},
                    "result":result
                }
            }),
        )?;
        Ok(json!({
            "type":"function_call_output",
            "call_id":call.call_id,
            "output":result.model_output()?
        }))
    }

    fn emit(&self, event_type: &str, payload: Value) -> Result<()> {
        self.events
            .emit(&event(event_type, payload))
            .map_err(boxed_error)
    }
}

fn boxed_error(error: BoxError) -> anyhow::Error {
    anyhow!(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_exec::CommandExecutor;
    use agent_policy::WorkspacePolicy;
    use agent_protocol::ModelResponse;
    use agent_store::MemoryEventSink;
    use async_trait::async_trait;
    use serde::Deserialize;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Deserialize)]
    struct Contract {
        prompt: String,
        responses: Vec<ModelResponse>,
        expected: Expected,
    }

    #[derive(Deserialize)]
    struct Expected {
        event_types: Vec<String>,
        item_types: Vec<String>,
        final_message: String,
        final_file_contains: String,
        model_request_count: usize,
        history_lengths: Vec<usize>,
    }

    struct ScriptedModel {
        responses: Mutex<std::vec::IntoIter<ModelResponse>>,
        histories: Mutex<Vec<Vec<Value>>>,
    }

    #[async_trait]
    impl ModelClient for ScriptedModel {
        async fn complete(
            &self,
            input_history: &[Value],
            _tool_specs: &[Value],
        ) -> Result<ModelResponse, BoxError> {
            self.histories.lock().unwrap().push(input_history.to_vec());
            self.responses
                .lock()
                .unwrap()
                .next()
                .ok_or_else(|| anyhow!("scripted response exhausted").into())
        }
    }

    #[tokio::test]
    async fn shared_contract_fixes_test_and_matches_events() {
        let contract_dir = contract_dir();
        let contract: Contract =
            serde_json::from_str(&fs::read_to_string(contract_dir.join("contract.json")).unwrap())
                .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        copy_directory(&contract_dir.join("initial"), temporary.path()).unwrap();
        let model = Arc::new(ScriptedModel {
            responses: Mutex::new(contract.responses.into_iter()),
            histories: Mutex::new(Vec::new()),
        });
        let events = Arc::new(MemoryEventSink::default());
        let policy = WorkspacePolicy::new(temporary.path()).unwrap();
        let tools = ToolRegistry::new(policy, CommandExecutor);
        let agent = Agent::new(model.clone(), tools, events.clone());

        let result = agent.run(&contract.prompt).await.unwrap();

        assert_eq!(result.final_message, contract.expected.final_message);
        assert!(
            fs::read_to_string(temporary.path().join("calculator.py"))
                .unwrap()
                .contains(&contract.expected.final_file_contains)
        );
        let recorded = events.events();
        let event_types: Vec<_> = recorded
            .iter()
            .map(|event| event["type"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(event_types, contract.expected.event_types);
        let item_types: Vec<_> = recorded
            .iter()
            .filter_map(|event| event.get("item"))
            .map(|item| item["type"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(item_types, contract.expected.item_types);
        let histories = model.histories.lock().unwrap();
        assert_eq!(histories.len(), contract.expected.model_request_count);
        assert_eq!(
            histories.iter().map(Vec::len).collect::<Vec<_>>(),
            contract.expected.history_lengths
        );
    }

    fn contract_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/fix-failing-test")
    }

    fn copy_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let target = destination.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                fs::create_dir_all(&target)?;
                copy_directory(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }
}
