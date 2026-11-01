use agent_context::{compact, limit_tool_result, validate_policy};
use agent_policy::{ApprovalMode, PolicyDecision};
use agent_protocol::{
    ApprovalDecision, ApprovalHandler, ApprovalRequest, BoxError, CancellationToken, ContextPolicy,
    EventSink, ModelClient, ToolCall, event,
};
use agent_store::{Record, SessionJournal, SessionState, TurnStatus};
use agent_tools::{ToolRegistry, ToolResult};
use anyhow::{Result, anyhow, ensure};
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
    context_policy: ContextPolicy,
}

#[derive(Debug)]
pub struct RunResult {
    pub thread_id: String,
    pub turn_id: String,
    pub final_message: String,
}

// Both ephemeral and durable runs use the same validated state machine.
struct Runtime {
    state: SessionState,
    journal: Option<Arc<SessionJournal>>,
}

impl Runtime {
    fn record(&mut self, record: Record) -> Result<()> {
        if let Some(journal) = &self.journal {
            journal.append(&record)?;
        }
        self.state.apply(&record)
    }

    fn ids(&self) -> Value {
        json!({"thread_id":self.state.thread_id,"turn_id":self.state.turn.as_ref().expect("active turn").id})
    }

    fn has_event(&self, kind: &str, item: Option<&str>) -> bool {
        self.state.events.iter().any(|e| {
            e["type"] == kind
                && (kind == "thread.started" || e["turn_id"] == self.ids()["turn_id"])
                && item.is_none_or(|id| e["item"]["id"] == id)
        })
    }
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
            context_policy: ContextPolicy::default(),
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
    pub fn with_context_policy(mut self, policy: ContextPolicy) -> Self {
        self.context_policy = policy;
        self
    }

    pub fn session_capabilities(&self) -> Value {
        json!({"tools":self.tools.capabilities(),"approval":format!("{:?}",self.approval_mode),"max_steps":self.max_steps})
    }

    pub async fn run(&self, prompt: &str) -> Result<RunResult> {
        self.run_session(Some(prompt), None).await
    }

    /// An interrupted turn resumes without a prompt. A terminal turn accepts a
    /// new prompt; a completed turn without one returns the recorded answer.
    pub async fn run_session(
        &self,
        prompt: Option<&str>,
        journal: Option<Arc<SessionJournal>>,
    ) -> Result<RunResult> {
        validate_policy(&self.context_policy)?;
        let mut rt = Runtime {
            state: match &journal {
                Some(j) => j.state()?,
                None => SessionState::default(),
            },
            journal,
        };
        let resumed = !rt.state.thread_id.is_empty() && rt.state.turn.is_some();
        if rt.state.thread_id.is_empty() {
            rt.record(Record::Created {
                thread_id: Uuid::new_v4().to_string(),
                workspace: self.tools.capabilities()["workspace"]
                    .as_str()
                    .unwrap_or("")
                    .into(),
                capabilities: self.session_capabilities(),
                context_policy: self.context_policy.clone(),
            })?;
        }
        ensure!(
            rt.state.capabilities == self.session_capabilities(),
            "resume capability mismatch: workspace, sandbox, allow-list and approval mode must match"
        );
        ensure!(
            rt.state.context_policy == self.context_policy,
            "resume context policy mismatch"
        );
        let running = rt
            .state
            .turn
            .as_ref()
            .is_some_and(|t| t.status == TurnStatus::Running);
        ensure!(
            !(running && prompt.is_some()),
            "interrupted turn must resume without a new prompt"
        );
        ensure!(
            rt.state.turn.is_some() || prompt.is_some(),
            "a new session requires a prompt"
        );
        if !rt.has_event("thread.started", None) {
            let payload = json!({"thread_id":rt.state.thread_id});
            self.emit(&mut rt, "thread.started", payload)?;
        }
        if resumed {
            self.repair_events(&mut rt)?;
            let payload = rt.ids();
            self.emit(&mut rt, "session.resumed", payload)?;
            if running {
                self.recover(&mut rt)?;
            }
        }
        if !running {
            if let Some(prompt) = prompt {
                rt.record(Record::TurnStarted {
                    turn_id: Uuid::new_v4().to_string(),
                    prompt: prompt.into(),
                })?;
            } else if rt
                .state
                .turn
                .as_ref()
                .is_some_and(|t| t.status == TurnStatus::Completed)
            {
                return self.result(&rt);
            } else {
                rt.record(Record::TurnStarted {turn_id:Uuid::new_v4().to_string(),prompt:
                    "Continue the previous task after interruption. Inspect the current workspace and recorded results first; do not assume prior changes were rolled back.".into()})?;
            }
        }
        if !rt.has_event("turn.started", None) {
            let payload = rt.ids();
            self.emit(&mut rt, "turn.started", payload)?;
        }
        match self.run_turn(&mut rt).await {
            Ok(()) if !self.cancellation.is_cancelled() => {
                rt.record(Record::TurnFinished {
                    status: TurnStatus::Completed,
                    error: None,
                })?;
                self.repair_events(&mut rt)?;
                self.result(&rt)
            }
            outcome => {
                let cancelled = self.cancellation.is_cancelled();
                let error = if cancelled {
                    anyhow!("turn cancelled")
                } else {
                    outcome.expect_err("success handled above")
                };
                // Close every announced call before ending a turn. No automatic rollback.
                while let Some(pending) = rt.state.pending.first().cloned() {
                    let result = if pending.started {
                        unknown_result()
                    } else {
                        json!({"success":false,"output":"Call was not executed because the turn ended.","cancelled":cancelled})
                    };
                    self.finish_tool(&mut rt, &pending.call, result)?;
                }
                rt.record(Record::TurnFinished {
                    status: if cancelled {
                        TurnStatus::Cancelled
                    } else {
                        TurnStatus::Failed
                    },
                    error: Some(error.to_string()),
                })?;
                self.repair_events(&mut rt)?;
                Err(error)
            }
        }
    }

    fn result(&self, rt: &Runtime) -> Result<RunResult> {
        let turn = rt.state.turn.as_ref().expect("active turn");
        Ok(RunResult {
            thread_id: rt.state.thread_id.clone(),
            turn_id: turn.id.clone(),
            final_message: turn
                .final_message
                .as_ref()
                .ok_or_else(|| anyhow!("final message missing"))?
                .text
                .clone(),
        })
    }

    fn recover(&self, rt: &mut Runtime) -> Result<()> {
        // An approval from the previous process is never reusable.
        let dangling: Vec<_> = rt
            .state
            .events
            .iter()
            .filter(|e| {
                e["type"] == "approval.requested"
                    && e["turn_id"] == rt.ids()["turn_id"]
                    && !rt.state.events.iter().any(|r| {
                        r["type"] == "approval.resolved" && r["approval_id"] == e["approval_id"]
                    })
            })
            .map(|e| e["approval_id"].clone())
            .collect();
        for id in dangling {
            let mut payload = rt.ids();
            payload["approval_id"] = id;
            payload["decision"] = json!("cancelled");
            payload["recovered"] = json!(true);
            self.emit(rt, "approval.resolved", payload)?;
        }
        let mut unknown = 0;
        while let Some(pending) = rt.state.pending.first().filter(|p| p.started).cloned() {
            self.finish_tool(rt, &pending.call, unknown_result())?;
            unknown += 1;
        }
        // Remaining calls from the same model response may depend on the unknown
        // side effect. Return them to the model as unexecuted instead of blindly
        // continuing a stale plan.
        if rt.state.recovery_required {
            while let Some(pending) = rt.state.pending.first().cloned() {
                self.finish_tool(rt, &pending.call, json!({"success":false,"execution_state":"not_executed","output":"Earlier execution in this batch has an unknown outcome. Inspect the workspace before issuing a new plan."}))?;
            }
        }
        let interrupted = rt
            .state
            .turn
            .as_ref()
            .is_some_and(|t| t.sampling_in_progress);
        if interrupted {
            rt.record(Record::SamplingInterrupted)?;
        }
        let mut payload = rt.ids();
        payload["unknown_tool_results"] = json!(unknown);
        payload["interrupted_sampling"] = json!(interrupted);
        self.emit(rt, "session.recovered", payload)
    }

    fn repair_events(&self, rt: &mut Runtime) -> Result<()> {
        if rt.state.turn.is_none() {
            return Ok(());
        }
        if !rt.has_event("turn.started", None) {
            let p = rt.ids();
            self.emit(rt, "turn.started", p)?;
        }
        for completed in rt.state.completed_tools.clone() {
            if !rt.has_event("item.completed", Some(&completed.call.call_id)) {
                if !rt.has_event("item.started", Some(&completed.call.call_id)) {
                    self.tool_event(rt, "item.started", &completed.call, None)?;
                }
                self.tool_event(
                    rt,
                    "item.completed",
                    &completed.call,
                    Some(completed.result),
                )?;
            }
        }
        if let Some(message) = rt.state.turn.as_ref().and_then(|t| t.final_message.clone())
            && !rt.has_event("item.completed", Some(&message.item_id))
        {
            let mut payload = rt.ids();
            payload["item"] =
                json!({"id":message.item_id,"type":"agent_message","text":message.text});
            self.emit(rt, "item.completed", payload)?;
        }
        let turn = rt.state.turn.as_ref().expect("active turn");
        let kind = match turn.status {
            TurnStatus::Running => return Ok(()),
            TurnStatus::Completed => "turn.completed",
            TurnStatus::Failed => "turn.failed",
            TurnStatus::Cancelled => "turn.cancelled",
        };
        if !rt.has_event(kind, None) {
            let mut payload = rt.ids();
            if turn.status == TurnStatus::Failed {
                payload["error"] = json!({"type":"runtime_error","message":turn.error});
            }
            self.emit(rt, kind, payload)?;
        }
        Ok(())
    }

    async fn run_turn(&self, rt: &mut Runtime) -> Result<()> {
        let specs = self.tools.specs();
        loop {
            ensure!(!self.cancellation.is_cancelled(), "turn cancelled");
            if let Some(pending) = rt.state.pending.first().cloned() {
                ensure!(!pending.started, "unrecovered execution intent");
                self.execute_call(rt, &pending.call).await?;
                continue;
            }
            if rt
                .state
                .turn
                .as_ref()
                .expect("active turn")
                .final_message
                .is_some()
            {
                self.repair_events(rt)?;
                return Ok(());
            }
            ensure!(
                rt.state.turn.as_ref().expect("active turn").sampling_steps < self.max_steps,
                "agent exceeded max_steps={}",
                self.max_steps
            );
            if let Some(view) = compact(
                &rt.state.history,
                &rt.state.request_history,
                &specs,
                &self.context_policy,
            )? {
                let mut payload = rt.ids();
                payload["before_tokens"] = json!(view.before_tokens);
                payload["after_tokens"] = json!(view.after_tokens);
                payload["summarized_pairs"] = json!(view.summarized_pairs);
                rt.record(Record::ContextCompacted {
                    history: view.history,
                    canonical_items: rt.state.history.len(),
                    before_tokens: view.before_tokens,
                    after_tokens: view.after_tokens,
                    summarized_pairs: view.summarized_pairs,
                })?;
                self.emit(rt, "context.compacted", payload)?;
            }
            rt.record(Record::SamplingStarted)?;
            let response = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(anyhow!("turn cancelled")),
                response = self.model.complete(&rt.state.request_history,&specs) => response.map_err(boxed_error)?,
            };
            rt.record(Record::ModelReturned {
                response,
                final_item_id: Uuid::new_v4().to_string(),
            })?;
        }
    }

    async fn execute_call(&self, rt: &mut Runtime, call: &ToolCall) -> Result<()> {
        if !rt.has_event("item.started", Some(&call.call_id)) {
            self.tool_event(rt, "item.started", call, None)?;
        }
        let mut approval_error = None;
        let allowed = match self
            .tools
            .decision(&call.name, &call.arguments, self.approval_mode)
        {
            PolicyDecision::Deny(reason) => {
                self.finish_tool(
                    rt,
                    call,
                    serde_json::to_value(ToolResult::error(format!("policy denied: {reason}")))?,
                )?;
                return Ok(());
            }
            PolicyDecision::Allow => true,
            PolicyDecision::Ask(reason) => {
                let request = ApprovalRequest {
                    approval_id: Uuid::new_v4().to_string(),
                    thread_id: rt.state.thread_id.clone(),
                    turn_id: rt.state.turn.as_ref().expect("active turn").id.clone(),
                    call_id: call.call_id.clone(),
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                    reason,
                    capabilities: self.tools.capabilities(),
                };
                self.emit(rt, "approval.requested", serde_json::to_value(&request)?)?;
                let decision = match &self.approvals {
                    Some(handler) => tokio::select! {
                        biased;
                        _ = self.cancellation.cancelled() => None,
                        decision = handler.approve(&request) => Some(match decision {Ok(d) => d,Err(e) => {approval_error=Some(e.to_string());ApprovalDecision::Deny}}),
                    },
                    None => Some(ApprovalDecision::Deny),
                };
                let mut payload = rt.ids();
                payload["approval_id"] = json!(request.approval_id);
                payload["decision"] = decision.map_or(json!("cancelled"), |d| json!(d));
                payload["error"] = json!(approval_error);
                self.emit(rt, "approval.resolved", payload)?;
                decision == Some(ApprovalDecision::Allow)
            }
        };
        let result = if allowed && !self.cancellation.is_cancelled() {
            rt.record(Record::ToolStarted {
                call_id: call.call_id.clone(),
            })?; // fsync before side effects.
            self.tools
                .execute_cancellable(&call.name, &call.arguments, &self.cancellation)
                .await
        } else {
            ToolResult::error(if self.cancellation.is_cancelled() {
                "approval cancelled"
            } else {
                "approval denied"
            })
        };
        self.finish_tool(rt, call, serde_json::to_value(result)?)?;
        if let Some(error) = approval_error {
            return Err(anyhow!(error));
        }
        Ok(())
    }

    fn finish_tool(&self, rt: &mut Runtime, call: &ToolCall, result: Value) -> Result<()> {
        if !rt.has_event("item.started", Some(&call.call_id)) {
            self.tool_event(rt, "item.started", call, None)?;
        }
        let model_output = limit_tool_result(&result, self.context_policy.tool_output_bytes)?;
        rt.record(Record::ToolFinished {
            call_id: call.call_id.clone(),
            result: result.clone(),
            model_output,
        })?;
        self.tool_event(rt, "item.completed", call, Some(result))
    }

    fn tool_event(
        &self,
        rt: &mut Runtime,
        kind: &str,
        call: &ToolCall,
        result: Option<Value>,
    ) -> Result<()> {
        let item_type = match call.name.as_str() {
            "exec_command" => "command_execution",
            "apply_patch" => "file_change",
            _ => "tool_call",
        };
        let mut payload = rt.ids();
        let mut item = json!({"id":call.call_id,"type":item_type,"name":call.name,"arguments":call.arguments,"status":"in_progress"});
        if let Some(result) = result {
            item["status"] = json!(if self.cancellation.is_cancelled()
                || result["cancelled"] == true
            {
                "cancelled"
            } else if result["success"] == true {
                "completed"
            } else {
                "failed"
            });
            item["result"] = result;
        }
        payload["item"] = item;
        self.emit(rt, kind, payload)
    }

    fn emit(&self, rt: &mut Runtime, kind: &str, payload: Value) -> Result<()> {
        let mut value = event(kind, payload);
        if rt.journal.is_some() {
            value["event_id"] = json!(Uuid::new_v4().to_string());
        }
        rt.record(Record::Event {
            event: value.clone(),
        })?;
        self.events.emit(&value).map_err(boxed_error)
    }
}

fn unknown_result() -> Value {
    json!({"success":false,"execution_state":"unknown","output":"The process ended after execution intent was saved but before a durable result. This call MUST NOT be replayed automatically. Inspect the workspace before deciding what to do next."})
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

    #[tokio::test]
    async fn recovery_at_durable_boundaries_repairs_events_and_never_resamples_final() {
        for boundary in ["planned", "started", "result", "final", "terminal"] {
            let root = tempfile::tempdir().unwrap();
            let workspace = root.path().join("workspace");
            fs::create_dir(&workspace).unwrap();
            fs::write(workspace.join("value.txt"), "original").unwrap();
            let model = Arc::new(ScriptedModel {
                responses: Mutex::new(
                    vec![ModelResponse {
                        text: Some("done".into()),
                        ..Default::default()
                    }]
                    .into_iter(),
                ),
                histories: Mutex::new(Vec::new()),
            });
            let events = Arc::new(MemoryEventSink::default());
            let agent = Agent::new(
                model.clone(),
                ToolRegistry::new(
                    WorkspacePolicy::new(&workspace).unwrap(),
                    CommandExecutor::default(),
                ),
                events.clone(),
            );
            let journal = Arc::new(
                SessionJournal::create(
                    &root.path().join("session.jsonl"),
                    Record::Created {
                        thread_id: "persistent-thread".into(),
                        workspace: workspace.to_string_lossy().into_owned(),
                        capabilities: agent.session_capabilities(),
                        context_policy: ContextPolicy::default(),
                    },
                )
                .unwrap(),
            );
            journal
                .append(&Record::TurnStarted {
                    turn_id: "persistent-turn".into(),
                    prompt: "read value".into(),
                })
                .unwrap();
            journal.append(&Record::SamplingStarted).unwrap();
            let call = ToolCall {
                call_id: "read-1".into(),
                name: "read_file".into(),
                arguments: json!({"path":"value.txt"}),
            };
            journal
                .append(&Record::ModelReturned {
                    response: ModelResponse {
                        tool_calls: vec![call.clone()],
                        ..Default::default()
                    },
                    final_item_id: "unused".into(),
                })
                .unwrap();
            if boundary != "planned" {
                journal
                    .append(&Record::ToolStarted {
                        call_id: call.call_id.clone(),
                    })
                    .unwrap();
            }
            if ["result", "final", "terminal"].contains(&boundary) {
                let result = json!({"success":true,"output":"original"});
                journal
                    .append(&Record::ToolFinished {
                        call_id: call.call_id.clone(),
                        result: result.clone(),
                        model_output: result.to_string(),
                    })
                    .unwrap();
            }
            if ["final", "terminal"].contains(&boundary) {
                journal.append(&Record::SamplingStarted).unwrap();
                journal
                    .append(&Record::ModelReturned {
                        response: ModelResponse {
                            text: Some("saved answer".into()),
                            ..Default::default()
                        },
                        final_item_id: "final-1".into(),
                    })
                    .unwrap();
            }
            if boundary == "terminal" {
                journal
                    .append(&Record::TurnFinished {
                        status: TurnStatus::Completed,
                        error: None,
                    })
                    .unwrap();
            }
            let result = agent
                .run_session(None, Some(journal.clone()))
                .await
                .unwrap();
            assert_eq!(result.thread_id, "persistent-thread");
            assert_eq!(result.turn_id, "persistent-turn");
            assert_eq!(
                model.histories.lock().unwrap().len(),
                if ["final", "terminal"].contains(&boundary) {
                    0
                } else {
                    1
                }
            );
            let state = journal.state().unwrap();
            agent_context::validate_history(&state.history, false).unwrap();
            assert_eq!(state.completed_tools.len(), 1);
            assert_eq!(
                state.completed_tools[0].result["execution_state"] == "unknown",
                boundary == "started"
            );
            let saved = events.events();
            assert_eq!(
                saved
                    .iter()
                    .filter(|e| e["type"] == "turn.completed")
                    .count(),
                1
            );
            assert_eq!(
                saved.iter().filter(|e| e["type"] == "item.started").count(),
                1
            );
            assert_eq!(
                saved
                    .iter()
                    .filter(|e| e["type"] == "item.completed" && e["item"]["id"] == "read-1")
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn recovery_barrier_survives_another_crash_before_batch_is_closed() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("value.txt"), "old\n").unwrap();
        let model = Arc::new(ScriptedModel {
            responses: Mutex::new(
                vec![ModelResponse {
                    text: Some("inspect first".into()),
                    ..Default::default()
                }]
                .into_iter(),
            ),
            histories: Mutex::new(Vec::new()),
        });
        let agent = Agent::new(
            model.clone(),
            ToolRegistry::new(
                WorkspacePolicy::new(&workspace).unwrap(),
                CommandExecutor::default(),
            ),
            Arc::new(MemoryEventSink::default()),
        )
        .with_approvals(ApprovalMode::Auto, None);
        let journal = Arc::new(
            SessionJournal::create(
                &root.path().join("session.jsonl"),
                Record::Created {
                    thread_id: "t".into(),
                    workspace: workspace.to_string_lossy().into_owned(),
                    capabilities: agent.session_capabilities(),
                    context_policy: ContextPolicy::default(),
                },
            )
            .unwrap(),
        );
        journal
            .append(&Record::TurnStarted {
                turn_id: "r".into(),
                prompt: "change".into(),
            })
            .unwrap();
        journal.append(&Record::SamplingStarted).unwrap();
        let calls = vec![
            ToolCall {
                call_id: "first".into(),
                name: "read_file".into(),
                arguments: json!({"path":"value.txt"}),
            },
            ToolCall {
                call_id: "second".into(),
                name: "apply_patch".into(),
                arguments: json!({"patch":"*** Begin Patch\n*** Update File: value.txt\n@@\n-old\n+new\n*** End Patch"}),
            },
        ];
        journal
            .append(&Record::ModelReturned {
                response: ModelResponse {
                    tool_calls: calls,
                    ..Default::default()
                },
                final_item_id: "unused".into(),
            })
            .unwrap();
        journal
            .append(&Record::ToolStarted {
                call_id: "first".into(),
            })
            .unwrap();
        let result = unknown_result();
        journal
            .append(&Record::ToolFinished {
                call_id: "first".into(),
                result: result.clone(),
                model_output: result.to_string(),
            })
            .unwrap();
        // Simulate another process death exactly after the unknown result was saved.
        let path = journal.path().to_path_buf();
        drop(journal);
        let journal = Arc::new(SessionJournal::open(&path).unwrap());
        agent
            .run_session(None, Some(journal.clone()))
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(workspace.join("value.txt")).unwrap(),
            "old\n"
        );
        assert_eq!(
            journal.state().unwrap().completed_tools[1].result["execution_state"],
            "not_executed"
        );
        assert_eq!(model.histories.lock().unwrap()[0].len(), 5);
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
