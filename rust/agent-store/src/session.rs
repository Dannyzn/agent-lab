use agent_context::{assistant_message, validate_history, validate_policy};
use agent_protocol::{ContextPolicy, ModelResponse, ToolCall};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FinalMessage {
    pub item_id: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TurnState {
    pub id: String,
    pub status: TurnStatus,
    pub sampling_steps: usize,
    pub sampling_in_progress: bool,
    pub final_message: Option<FinalMessage>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolCompletion {
    pub call: ToolCall,
    pub result: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PendingTool {
    pub call: ToolCall,
    pub started: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub thread_id: String,
    pub workspace: String,
    pub capabilities: Value,
    pub context_policy: ContextPolicy,
    pub history: Vec<Value>,
    pub request_history: Vec<Value>,
    pub pending: Vec<PendingTool>,
    pub completed_tools: Vec<ToolCompletion>,
    pub turn: Option<TurnState>,
    pub events: Vec<Value>,
    pub compactions: usize,
    pub recovery_required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Record {
    Created {
        thread_id: String,
        workspace: String,
        capabilities: Value,
        context_policy: ContextPolicy,
    },
    TurnStarted {
        turn_id: String,
        prompt: String,
    },
    SamplingStarted,
    SamplingInterrupted,
    ModelReturned {
        response: ModelResponse,
        final_item_id: String,
    },
    ToolStarted {
        call_id: String,
    },
    ToolFinished {
        call_id: String,
        result: Value,
        model_output: String,
    },
    ContextCompacted {
        history: Vec<Value>,
        canonical_items: usize,
        before_tokens: usize,
        after_tokens: usize,
        summarized_pairs: usize,
    },
    TurnFinished {
        status: TurnStatus,
        error: Option<String>,
    },
    Event {
        event: Value,
    },
}

impl SessionState {
    pub fn apply(&mut self, record: &Record) -> Result<()> {
        match record {
            Record::Created {
                thread_id,
                workspace,
                capabilities,
                context_policy,
            } => {
                ensure!(self.thread_id.is_empty(), "session already created");
                ensure!(
                    !thread_id.is_empty() && !workspace.is_empty(),
                    "session identity missing"
                );
                validate_policy(context_policy)?;
                self.thread_id = thread_id.clone();
                self.workspace = workspace.clone();
                self.capabilities = capabilities.clone();
                self.context_policy = context_policy.clone();
            }
            Record::TurnStarted { turn_id, prompt } => {
                ensure!(!self.thread_id.is_empty(), "session not created");
                ensure!(
                    self.turn
                        .as_ref()
                        .is_none_or(|turn| turn.status != TurnStatus::Running),
                    "turn is already running"
                );
                ensure!(self.pending.is_empty(), "pending tools at turn boundary");
                ensure!(!turn_id.is_empty(), "turn id is empty");
                let message = json!({"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]});
                self.history.push(message.clone());
                self.request_history.push(message);
                self.completed_tools.clear();
                self.recovery_required = false;
                self.turn = Some(TurnState {
                    id: turn_id.clone(),
                    status: TurnStatus::Running,
                    sampling_steps: 0,
                    sampling_in_progress: false,
                    final_message: None,
                    error: None,
                });
            }
            Record::SamplingStarted => {
                ensure!(self.pending.is_empty(), "cannot sample with pending tools");
                validate_history(&self.request_history, false)?;
                self.recovery_required = false;
                let turn = self.running_mut()?;
                ensure!(
                    !turn.sampling_in_progress && turn.final_message.is_none(),
                    "sampling already active or final message recorded"
                );
                turn.sampling_steps += 1;
                turn.sampling_in_progress = true;
            }
            Record::SamplingInterrupted => {
                self.running_mut()?.sampling_in_progress = false;
            }
            Record::ModelReturned {
                response,
                final_item_id,
            } => {
                ensure!(
                    self.pending.is_empty(),
                    "model response while tools pending"
                );
                ensure!(
                    self.running_mut()?.sampling_in_progress,
                    "unsolicited model response"
                );
                ensure!(
                    !response.tool_calls.is_empty() || response.text.is_some(),
                    "empty model response"
                );
                let mut ids: HashSet<String> = self
                    .history
                    .iter()
                    .filter(|item| item["type"] == "function_call")
                    .filter_map(|item| item["call_id"].as_str().map(str::to_owned))
                    .collect();
                for call in &response.tool_calls {
                    ensure!(
                        !call.call_id.is_empty()
                            && !call.name.is_empty()
                            && ids.insert(call.call_id.clone()),
                        "duplicate or empty model tool call id/name"
                    );
                }
                self.running_mut()?.sampling_in_progress = false;
                if let Some(text) = &response.text {
                    let message = assistant_message(text);
                    self.history.push(message.clone());
                    self.request_history.push(message);
                    if response.tool_calls.is_empty() {
                        self.running_mut()?.final_message = Some(FinalMessage {
                            item_id: final_item_id.clone(),
                            text: text.clone(),
                        });
                    }
                }
                self.pending = response
                    .tool_calls
                    .iter()
                    .cloned()
                    .map(|call| PendingTool {
                        call,
                        started: false,
                    })
                    .collect();
            }
            Record::ToolStarted { call_id } => {
                self.running_mut()?;
                let pending = self.pending.first().context("no pending tool")?;
                ensure!(
                    &pending.call.call_id == call_id && !pending.started,
                    "tool intent is out of order or duplicated"
                );
                let call = pending.call.clone();
                self.append_call(&call)?;
                self.pending[0].started = true;
            }
            Record::ToolFinished {
                call_id,
                result,
                model_output,
            } => {
                self.running_mut()?;
                let pending = self.pending.first().context("no pending tool result")?;
                ensure!(
                    &pending.call.call_id == call_id,
                    "tool result is out of order or duplicated"
                );
                let projected: Value = serde_json::from_str(model_output)?;
                ensure!(
                    result["success"].is_boolean() && projected["success"] == result["success"],
                    "invalid tool result projection"
                );
                let call = pending.call.clone();
                if !pending.started {
                    self.append_call(&call)?;
                }
                if result["execution_state"] == "unknown" {
                    self.recovery_required = true;
                }
                self.completed_tools.push(ToolCompletion {
                    call,
                    result: result.clone(),
                });
                self.history.push(json!({"type":"function_call_output","call_id":call_id,"output":serde_json::to_string(result)?}));
                self.request_history.push(
                    json!({"type":"function_call_output","call_id":call_id,"output":model_output}),
                );
                self.pending.remove(0);
            }
            Record::ContextCompacted {
                history,
                canonical_items,
                ..
            } => {
                self.running_mut()?;
                ensure!(
                    self.pending.is_empty() && *canonical_items == self.history.len(),
                    "compaction source mismatch"
                );
                validate_history(history, false)?;
                let users = |items: &[Value]| {
                    items
                        .iter()
                        .filter(|item| item["type"] == "message" && item["role"] == "user")
                        .cloned()
                        .collect::<Vec<_>>()
                };
                ensure!(
                    users(history) == users(&self.history),
                    "compaction changed user instructions"
                );
                self.request_history = history.clone();
                self.compactions += 1;
            }
            Record::TurnFinished { status, error } => {
                ensure!(self.pending.is_empty(), "cannot finish with pending tools");
                validate_history(&self.history, false)?;
                let turn = self.running_mut()?;
                ensure!(*status != TurnStatus::Running, "invalid terminal status");
                ensure!(
                    *status != TurnStatus::Completed || turn.final_message.is_some(),
                    "completed turn missing final message"
                );
                turn.status = *status;
                turn.error = error.clone();
                turn.sampling_in_progress = false;
            }
            Record::Event { event } => {
                ensure!(
                    event["thread_id"] == self.thread_id,
                    "event belongs to another session"
                );
                if let Some(id) = event.get("turn_id") {
                    ensure!(
                        self.turn.as_ref().is_some_and(|turn| id == &turn.id),
                        "event belongs to another turn"
                    );
                }
                self.events.push(event.clone());
            }
        }
        Ok(())
    }

    fn running_mut(&mut self) -> Result<&mut TurnState> {
        let turn = self.turn.as_mut().context("no active turn")?;
        ensure!(turn.status == TurnStatus::Running, "turn is terminal");
        Ok(turn)
    }

    fn append_call(&mut self, call: &ToolCall) -> Result<()> {
        let item = json!({"type":"function_call","call_id":call.call_id,"name":call.name,"arguments":serde_json::to_string(&call.arguments)?});
        self.history.push(item.clone());
        self.request_history.push(item);
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct Envelope {
    version: u32,
    seq: u64,
    record: Record,
}

struct Inner {
    file: File,
    state: SessionState,
    next_seq: u64,
    failed: bool,
}

pub struct SessionJournal {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl SessionJournal {
    pub fn create(path: &Path, record: Record) -> Result<Self> {
        ensure!(
            matches!(record, Record::Created { .. }),
            "first record must create session"
        );
        SessionState::default().apply(&record)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut options = OpenOptions::new();
        options.read(true).append(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .context("cannot create session rollout (existing files are never replaced)")?;
        lock(&file)?;
        File::open(parent)?.sync_all()?;
        let journal = Self {
            path: path.to_path_buf(),
            inner: Mutex::new(Inner {
                file,
                state: SessionState::default(),
                next_seq: 0,
                failed: false,
            }),
        };
        journal.append(&record)?;
        Ok(journal)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path).context("cannot open session rollout")?;
        lock(&file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                file.metadata()?.permissions().mode() & 0o077 == 0,
                "session rollout permissions must be private (chmod 600)"
            );
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let valid_end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1);
        ensure!(valid_end > 0, "session has no complete records");
        let mut state = SessionState::default();
        let mut next_seq = 0;
        for line in bytes[..valid_end - 1].split(|byte| *byte == b'\n') {
            let envelope: Envelope =
                serde_json::from_slice(line).context("corrupt complete rollout record")?;
            ensure!(
                envelope.version == 1 && envelope.seq == next_seq,
                "unsupported rollout version or sequence gap"
            );
            state
                .apply(&envelope.record)
                .context("invalid rollout state transition")?;
            next_seq += 1;
        }
        ensure!(
            !state.thread_id.is_empty(),
            "rollout missing session identity"
        );
        if valid_end != bytes.len() {
            file.set_len(valid_end as u64)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            path: path.to_path_buf(),
            inner: Mutex::new(Inner {
                file,
                state,
                next_seq,
                failed: false,
            }),
        })
    }

    pub fn append(&self, record: &Record) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("session lock poisoned"))?;
        ensure!(
            !inner.failed,
            "session journal is unusable after a write failure; reopen to recover"
        );
        let mut state = inner.state.clone();
        state.apply(record)?; // Validate before making the record durable.
        let mut bytes = serde_json::to_vec(&Envelope {
            version: 1,
            seq: inner.next_seq,
            record: record.clone(),
        })?;
        bytes.push(b'\n');
        inner.failed = true;
        inner.file.write_all(&bytes)?;
        inner.file.sync_data()?; // The caller must not execute a side effect before this returns.
        inner.failed = false;
        inner.state = state;
        inner.next_seq += 1;
        Ok(())
    }

    pub fn state(&self) -> Result<SessionState> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("session lock poisoned"))?
            .state
            .clone())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn lock(file: &File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: flock operates on the valid owned descriptor and releases at close.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!(
                "session is locked by another writer: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn created() -> Record {
        Record::Created {
            thread_id: "thread-1".into(),
            workspace: "/tmp/workspace".into(),
            capabilities: json!({}),
            context_policy: ContextPolicy::default(),
        }
    }

    #[test]
    fn journal_is_private_locked_and_recovers_torn_tail() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.jsonl");
        let journal = SessionJournal::create(&path, created()).unwrap();
        journal
            .append(&Record::TurnStarted {
                turn_id: "turn-1".into(),
                prompt: "fix".into(),
            })
            .unwrap();
        assert!(SessionJournal::open(&path).is_err());
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"version\":1")
            .unwrap();
        let reopened = SessionJournal::open(&path).unwrap();
        assert_eq!(reopened.state().unwrap().history.len(), 1);
        assert!(std::fs::read(&path).unwrap().ends_with(b"\n"));
    }

    #[test]
    fn complete_corruption_and_duplicate_results_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.jsonl");
        let journal = SessionJournal::create(&path, created()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(
            journal
                .append(&Record::ToolStarted {
                    call_id: "absent".into()
                })
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"invalid complete record\n")
            .unwrap();
        assert!(SessionJournal::open(&path).is_err());
    }
}
