use agent_exec::{CommandExecutor, DEFAULT_MAX_OUTPUT_BYTES, ExecRequest};
use agent_kernel::Agent;
use agent_policy::{ApprovalMode, WorkspacePolicy};
use agent_protocol::{
    ApprovalDecision, ApprovalHandler, ApprovalRequest, BoxError, CancellationToken, EventSink,
};
use agent_sandbox::{FilesystemAccess, NetworkAccess, SandboxConfig};
use agent_store::{
    JsonlEventSink, NullEventSink, Record, SessionJournal, StdoutEventSink, TeeEventSink,
};
use agent_tools::ToolRegistry;
use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use provider_openai::OpenAIResponsesClient;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ApprovalArg {
    Ask,
    Never,
    Auto,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum SandboxArg {
    Seatbelt,
    None,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum FilesystemArg {
    ReadOnly,
    WorkspaceWrite,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum NetworkArg {
    Deny,
    Allow,
}

#[derive(Parser, Debug)]
#[command(name = "our-agent", version, about = "A small coding-agent runtime")]
struct Arguments {
    /// Coding task to execute.
    prompt: Option<String>,
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Create a private durable session outside the tool workspace.
    #[arg(long, conflicts_with = "resume")]
    session: Option<PathBuf>,
    /// Resume an interrupted session, or add a prompt to a completed session.
    #[arg(long)]
    resume: Option<PathBuf>,
    /// Conservative local context estimate, including tools and output reserve.
    #[arg(long)]
    context_tokens: Option<usize>,
    #[arg(long)]
    tool_output_bytes: Option<usize>,
    #[arg(long, default_value = "gpt-5.5")]
    model: String,
    #[arg(long, default_value = "https://api.openai.com/v1")]
    base_url: String,
    /// Optional provider output-token limit per request.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max_output_tokens: Option<u32>,
    /// Optional Responses reasoning effort supported by the selected provider.
    #[arg(long)]
    reasoning_effort: Option<String>,
    /// Append runtime events to this JSONL file.
    #[arg(long)]
    events: Option<PathBuf>,
    /// JSONL events on stdout; approvals use correlated JSON lines on stdin.
    #[arg(long)]
    json: bool,
    #[arg(long = "allow-command")]
    allowed_commands: Vec<String>,
    #[arg(long)]
    max_steps: Option<usize>,
    /// Ask before patches/commands; never denies them; auto permits within policy.
    #[arg(long, value_enum, default_value = "ask")]
    approval: ApprovalArg,
    #[arg(long, default_value_t = 120_000, value_parser = clap::value_parser!(u64).range(1..=600_000))]
    approval_timeout_ms: u64,
    /// OS enforcement. none is an explicit unsafe development opt-out.
    #[arg(long, value_enum, default_value = "seatbelt")]
    sandbox: SandboxArg,
    #[arg(long, value_enum, default_value = "workspace-write")]
    filesystem: FilesystemArg,
    /// Applies to tool subprocesses, not the host's model API connection.
    #[arg(long, value_enum, default_value = "deny")]
    network: NetworkArg,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let loaded = arguments
        .resume
        .as_deref()
        .map(SessionJournal::open)
        .transpose()?
        .map(Arc::new);
    let saved = loaded.as_ref().map(|j| j.state()).transpose()?;
    let workspace = arguments
        .workspace
        .clone()
        .or_else(|| saved.as_ref().map(|s| PathBuf::from(&s.workspace)))
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()
        .context("cannot resolve workspace")?;
    let session_path = arguments
        .session
        .as_deref()
        .or(arguments.resume.as_deref())
        .map(|p| outside_path(p, &workspace, "session rollout"))
        .transpose()?;
    let event_path = arguments
        .events
        .as_deref()
        .map(|p| outside_path(p, &workspace, "event log"))
        .transpose()?;
    anyhow::ensure!(
        session_path.is_none() || session_path != event_path,
        "session rollout and event log must be different files"
    );
    anyhow::ensure!(
        arguments.prompt.is_some() || loaded.is_some(),
        "a new session requires a prompt"
    );
    let mut context_policy = saved
        .as_ref()
        .map(|s| s.context_policy.clone())
        .unwrap_or_default();
    if let Some(limit) = arguments.context_tokens {
        context_policy.max_tokens = limit;
    }
    if let Some(limit) = arguments.tool_output_bytes {
        context_policy.tool_output_bytes = limit;
    }
    if let Some(limit) = arguments.max_output_tokens {
        context_policy.reserve_output_tokens = limit as usize;
    }
    let output_limit = u32::try_from(context_policy.reserve_output_tokens)
        .context("output reserve is too large")?;
    let sandbox = SandboxConfig {
        enabled: matches!(arguments.sandbox, SandboxArg::Seatbelt),
        filesystem: match arguments.filesystem {
            FilesystemArg::ReadOnly => FilesystemAccess::ReadOnly,
            FilesystemArg::WorkspaceWrite => FilesystemAccess::WorkspaceWrite,
        },
        network: match arguments.network {
            NetworkArg::Deny => NetworkAccess::Deny,
            NetworkArg::Allow => NetworkAccess::Allow,
        },
    };
    if !sandbox.enabled {
        eprintln!(
            "Warning: OS sandbox disabled; subprocess file/network capabilities are unenforced."
        );
    }
    let executor = CommandExecutor::new(sandbox.clone())?;
    let policy = if arguments.allowed_commands.is_empty() {
        WorkspacePolicy::new(&workspace)?
    } else {
        WorkspacePolicy::with_commands(&workspace, arguments.allowed_commands)?
    }
    .with_read_only(sandbox.filesystem == FilesystemAccess::ReadOnly);
    let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY is required")?;
    let model = Arc::new(
        OpenAIResponsesClient::new(arguments.model, api_key, arguments.base_url)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
            .with_generation_options(Some(output_limit), arguments.reasoning_effort),
    );
    let tools = ToolRegistry::new(policy, executor.clone());
    let events = event_sink(arguments.json, event_path.as_deref())?;
    let mode = match arguments.approval {
        ApprovalArg::Ask => ApprovalMode::Ask,
        ApprovalArg::Never => ApprovalMode::Never,
        ApprovalArg::Auto => ApprovalMode::Auto,
    };
    let cancellation = CancellationToken::default();
    let signal_task = tokio::spawn(watch_signals(cancellation.clone()));
    let agent = Agent::new(model, tools, events)
        .with_max_steps(arguments.max_steps.unwrap_or_else(|| {
            saved
                .as_ref()
                .and_then(|s| s.capabilities["max_steps"].as_u64())
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(16)
        }))
        .with_approvals(
            mode,
            Some(Arc::new(StdinApproval {
                json: arguments.json,
                timeout_ms: arguments.approval_timeout_ms,
            })),
        )
        .with_cancellation(cancellation.clone())
        .with_context_policy(context_policy.clone());
    let journal = if let Some(loaded) = loaded {
        Some(loaded)
    } else if let Some(path) = session_path {
        Some(Arc::new(SessionJournal::create(
            &path,
            Record::Created {
                thread_id: uuid::Uuid::new_v4().to_string(),
                workspace: workspace.to_string_lossy().into_owned(),
                capabilities: agent.session_capabilities(),
                context_policy,
            },
        )?))
    } else {
        None
    };
    let result = agent
        .run_session(arguments.prompt.as_deref(), journal)
        .await;
    if cancellation.is_cancelled() {
        std::process::exit(130);
    }
    let result = result?;
    if !arguments.json {
        println!("{}", result.final_message);
        let diff = print_git_diff(&workspace, sandbox, &cancellation).await;
        if cancellation.is_cancelled() {
            std::process::exit(130);
        }
        diff?;
    }
    signal_task.abort();
    if cancellation.is_cancelled() {
        std::process::exit(130);
    }
    Ok(())
}

struct StdinApproval {
    json: bool,
    timeout_ms: u64,
}

#[async_trait]
impl ApprovalHandler for StdinApproval {
    async fn approve(&self, request: &ApprovalRequest) -> Result<ApprovalDecision, BoxError> {
        if !self.json {
            eprintln!(
                "\nApproval {}: {}\nArguments: {}\nCapabilities: {}\nAllow this call once? [y/N]",
                request.approval_id, request.tool, request.arguments, request.capabilities
            );
            std::io::stderr().flush()?;
        }
        let json = self.json;
        let expected = request.approval_id.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // Detached OS thread: cancelling stdin must not hold Tokio runtime shutdown open.
        std::thread::spawn(move || {
            let mut line = String::new();
            let decision = if std::io::stdin().read_line(&mut line).is_err() {
                ApprovalDecision::Deny
            } else if json {
                serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .filter(|reply| reply["approval_id"] == expected)
                    .and_then(|reply| serde_json::from_value(reply["decision"].clone()).ok())
                    .unwrap_or(ApprovalDecision::Deny)
            } else if matches!(line.trim(), "y" | "Y" | "yes") {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Deny
            };
            let _ = sender.send(decision);
        });
        // A timed-out reader is still detached, so terminate this turn instead of
        // asking again and racing two readers for a later approval response.
        match tokio::time::timeout(Duration::from_millis(self.timeout_ms), receiver).await {
            Ok(Ok(decision)) => Ok(decision),
            _ => Err(anyhow::anyhow!("approval input unavailable or timed out").into()),
        }
    }
}

async fn watch_signals(cancellation: CancellationToken) {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            cancellation.cancel();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        cancellation.cancel();
    }
}

fn event_sink(json: bool, path: Option<&std::path::Path>) -> Result<Arc<dyn EventSink>> {
    let mut sinks: Vec<Arc<dyn EventSink>> = Vec::new();
    if json {
        sinks.push(Arc::new(StdoutEventSink));
    }
    if let Some(path) = path {
        sinks.push(Arc::new(JsonlEventSink::open(path)?));
    }
    Ok(match sinks.len() {
        0 => Arc::new(NullEventSink),
        1 => sinks.pop().expect("one sink"),
        _ => Arc::new(TeeEventSink::new(sinks)),
    })
}

async fn print_git_diff(
    workspace: &std::path::Path,
    mut sandbox: SandboxConfig,
    cancellation: &CancellationToken,
) -> Result<()> {
    sandbox.filesystem = FilesystemAccess::ReadOnly;
    sandbox.network = NetworkAccess::Deny;
    let result = CommandExecutor::new(sandbox)?
        .run_cancellable(
            ExecRequest {
                argv: vec![
                    "git".into(),
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                ],
                cwd: workspace.to_path_buf(),
                timeout_ms: 10_000,
                max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            },
            cancellation,
        )
        .await?;
    if result.success && !result.output.is_empty() {
        println!("\nDiff:\n{}", result.output);
    }
    Ok(())
}

fn outside_path(
    path: &std::path::Path,
    workspace: &std::path::Path,
    label: &str,
) -> Result<PathBuf> {
    let resolved = if path.exists() {
        path.canonicalize()?
    } else {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;
        parent
            .canonicalize()?
            .join(path.file_name().context("log path must be a file")?)
    };
    anyhow::ensure!(
        !resolved.starts_with(workspace),
        "{label} must be outside the tool workspace to prevent tool tampering"
    );
    Ok(resolved)
}
