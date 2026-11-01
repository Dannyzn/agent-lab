use agent_policy::{ApprovalMode, PolicyDecision};
use agent_protocol::{
    ApprovalDecision, ApprovalHandler, ApprovalRequest, BoxError, CancellationToken, EventSink,
    ModelClient, ToolCall, event,
};
use agent_tools::{ToolRegistry, ToolResult};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub struct Agent {
    model: Arc<dyn ModelClient>,
    tools: ToolRegistry,
    events: Arc<dyn EventSink>,
    max_steps: usize,
    approval_mode: ApprovalMode,
    approvals: Option<Arc<dyn ApprovalHandler>>,
    cancellation: CancellationToken,
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
            approval_mode: ApprovalMode::Ask,
            approvals: None,
            cancellation: CancellationToken::default(),
        }
    }

    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    pub fn with_approvals(
        mut self,
        mode: ApprovalMode,
        handler: Option<Arc<dyn ApprovalHandler>>,
    ) -> Self {
        self.approval_mode = mode;
        self.approvals = handler;
        self
    }

    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
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
            Ok(final_message) if !self.cancellation.is_cancelled() => {
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
            result => {
                if self.cancellation.is_cancelled() {
                    self.emit(
                        "turn.cancelled",
                        json!({"thread_id":thread_id,"turn_id":turn_id}),
                    )?;
                    return Err(anyhow!("turn cancelled"));
                }
                let error = result.expect_err("successful result handled above");
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
            let response = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(anyhow!("turn cancelled")),
                response = self.model.complete(&history, &tool_specs) => response.map_err(boxed_error)?,
            };
            if !response.tool_calls.is_empty() {
                for call in response.tool_calls {
                    anyhow::ensure!(!self.cancellation.is_cancelled(), "turn cancelled");
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
        let mut approval_error = None;
        let result = match self
            .tools
            .decision(&call.name, &call.arguments, self.approval_mode)
        {
            PolicyDecision::Deny(reason) => ToolResult::error(format!("policy denied: {reason}")),
            PolicyDecision::Allow => {
                self.tools
                    .execute_cancellable(&call.name, &call.arguments, &self.cancellation)
                    .await
            }
            PolicyDecision::Ask(reason) => {
                let request = ApprovalRequest {
                    approval_id: Uuid::new_v4().to_string(),
                    thread_id: thread_id.into(),
                    turn_id: turn_id.into(),
                    call_id: call.call_id.clone(),
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                    reason,
                    capabilities: self.tools.capabilities(),
                };
                self.emit("approval.requested", serde_json::to_value(&request)?)?;
                let decision = match &self.approvals {
                    Some(handler) => tokio::select! {
                        biased;
                        _ = self.cancellation.cancelled() => {
                            self.emit("approval.resolved", json!({"thread_id":thread_id,"turn_id":turn_id,"approval_id":request.approval_id,"decision":"cancelled"}))?;
                            None
                        },
                        decision = handler.approve(&request) => Some(match decision {
                            Ok(decision) => decision,
                            Err(error) => { approval_error = Some(error.to_string()); ApprovalDecision::Deny }
                        }),
                    },
                    None => Some(ApprovalDecision::Deny),
                };
                if let Some(decision) = decision {
                    self.emit("approval.resolved", json!({"thread_id":thread_id,"turn_id":turn_id,"approval_id":request.approval_id,"decision":decision,"error":approval_error}))?;
                }
                if decision == Some(ApprovalDecision::Allow) && !self.cancellation.is_cancelled() {
                    self.tools
                        .execute_cancellable(&call.name, &call.arguments, &self.cancellation)
                        .await
                } else {
                    ToolResult::error(if decision.is_none() {
                        "approval cancelled"
                    } else {
                        "approval denied"
                    })
                }
            }
        };
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
                    "status":if self.cancellation.is_cancelled() {"cancelled"} else if result.success {"completed"} else {"failed"},
                    "result":result
                }
            }),
        )?;
        if let Some(error) = approval_error {
            return Err(anyhow!(error));
        }
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
        let tools = ToolRegistry::new(policy, CommandExecutor::default());
        let agent = Agent::new(model.clone(), tools, events.clone())
            .with_approvals(ApprovalMode::Auto, None);

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

    struct FixedApproval(ApprovalDecision);
    #[async_trait]
    impl ApprovalHandler for FixedApproval {
        async fn approve(&self, _request: &ApprovalRequest) -> Result<ApprovalDecision, BoxError> {
            Ok(self.0)
        }
    }

    struct PendingApproval;
    #[async_trait]
    impl ApprovalHandler for PendingApproval {
        async fn approve(&self, _request: &ApprovalRequest) -> Result<ApprovalDecision, BoxError> {
            std::future::pending().await
        }
    }

    struct PendingModel;
    #[async_trait]
    impl ModelClient for PendingModel {
        async fn complete(&self, _: &[Value], _: &[Value]) -> Result<ModelResponse, BoxError> {
            std::future::pending().await
        }
    }

    fn patch_model() -> Arc<ScriptedModel> {
        Arc::new(ScriptedModel {
            responses: Mutex::new(vec![ModelResponse {
                tool_calls: vec![ToolCall { call_id: "patch-1".into(), name: "apply_patch".into(), arguments: json!({"patch":"*** Begin Patch\n*** Update File: value.txt\n@@\n-old\n+new\n*** End Patch"}) }], ..Default::default()
            }, ModelResponse { text: Some("done".into()), ..Default::default() }].into_iter()),
            histories: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn approval_is_once_per_call_and_denial_returns_to_model() {
        for decision in [ApprovalDecision::Allow, ApprovalDecision::Deny] {
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join("value.txt"), "old\n").unwrap();
            let model = patch_model();
            let events = Arc::new(MemoryEventSink::default());
            let tools = ToolRegistry::new(
                WorkspacePolicy::new(root.path()).unwrap(),
                CommandExecutor::default(),
            );
            let agent = Agent::new(model.clone(), tools, events.clone())
                .with_approvals(ApprovalMode::Ask, Some(Arc::new(FixedApproval(decision))));
            agent.run("change value").await.unwrap();
            assert_eq!(
                fs::read_to_string(root.path().join("value.txt")).unwrap(),
                if decision == ApprovalDecision::Allow {
                    "new\n"
                } else {
                    "old\n"
                }
            );
            let recorded = events.events();
            assert_eq!(
                recorded
                    .iter()
                    .filter(|e| e["type"] == "approval.requested")
                    .count(),
                1
            );
            assert_eq!(
                recorded
                    .iter()
                    .filter(|e| e["type"] == "approval.resolved")
                    .count(),
                1
            );
            let histories = model.histories.lock().unwrap();
            assert_eq!(histories[1].len(), 3);
            let output: Value =
                serde_json::from_str(histories[1][2]["output"].as_str().unwrap()).unwrap();
            assert_eq!(output["success"], decision == ApprovalDecision::Allow);
        }
    }

    #[tokio::test]
    async fn readonly_cannot_be_overridden_by_auto_approval() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("value.txt"), "old\n").unwrap();
        let events = Arc::new(MemoryEventSink::default());
        let tools = ToolRegistry::new(
            WorkspacePolicy::new(root.path())
                .unwrap()
                .with_read_only(true),
            CommandExecutor::default(),
        );
        Agent::new(patch_model(), tools, events.clone())
            .with_approvals(ApprovalMode::Auto, None)
            .run("change")
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("value.txt")).unwrap(),
            "old\n"
        );
        assert!(
            !events
                .events()
                .iter()
                .any(|e| e["type"] == "approval.requested")
        );
    }

    #[tokio::test]
    async fn cancellation_while_sampling_or_awaiting_approval_is_terminal() {
        for awaiting in [false, true] {
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join("value.txt"), "old\n").unwrap();
            let events = Arc::new(MemoryEventSink::default());
            let cancellation = CancellationToken::default();
            let model: Arc<dyn ModelClient> = if awaiting {
                patch_model()
            } else {
                Arc::new(PendingModel)
            };
            let tools = ToolRegistry::new(
                WorkspacePolicy::new(root.path()).unwrap(),
                CommandExecutor::default(),
            );
            let agent = Agent::new(model, tools, events.clone())
                .with_approvals(ApprovalMode::Ask, Some(Arc::new(PendingApproval)))
                .with_cancellation(cancellation.clone());
            let task = tokio::spawn(async move { agent.run("change").await });
            for _ in 0..100 {
                if events.events().iter().any(|e| {
                    e["type"]
                        == if awaiting {
                            "approval.requested"
                        } else {
                            "turn.started"
                        }
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            cancellation.cancel();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            let recorded = events.events();
            assert_eq!(recorded.last().unwrap()["type"], "turn.cancelled");
            assert!(
                !recorded
                    .iter()
                    .any(|e| e["type"] == "turn.completed" || e["type"] == "turn.failed")
            );
            assert_eq!(
                fs::read_to_string(root.path().join("value.txt")).unwrap(),
                "old\n"
            );
            if awaiting {
                assert!(
                    recorded
                        .iter()
                        .any(|e| e["type"] == "approval.resolved" && e["decision"] == "cancelled")
                );
                assert!(
                    recorded.iter().any(
                        |e| e["type"] == "item.completed" && e["item"]["status"] == "cancelled"
                    )
                );
            }
        }
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
