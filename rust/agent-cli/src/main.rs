use agent_exec::CommandExecutor;
use agent_kernel::Agent;
use agent_policy::WorkspacePolicy;
use agent_protocol::EventSink;
use agent_store::{JsonlEventSink, NullEventSink, StdoutEventSink, TeeEventSink};
use agent_tools::ToolRegistry;
use anyhow::{Context, Result};
use clap::Parser;
use provider_openai::OpenAIResponsesClient;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

#[derive(Parser, Debug)]
#[command(name = "our-agent", version, about = "A small coding-agent runtime")]
struct Arguments {
    /// Coding task to execute.
    prompt: String,

    /// Workspace root available to file tools and commands.
    #[arg(long, default_value = ".")]
    workspace: PathBuf,

    /// OpenAI Responses model.
    #[arg(long, default_value = "gpt-5.5")]
    model: String,

    /// Responses-compatible API base URL.
    #[arg(long, default_value = "https://api.openai.com/v1")]
    base_url: String,

    /// Append public runtime events to this JSONL file.
    #[arg(long)]
    events: Option<PathBuf>,

    /// Stream public runtime events to stdout as JSONL.
    #[arg(long)]
    json: bool,

    /// Replace the default executable allow-list. Repeat for each command.
    #[arg(long = "allow-command")]
    allowed_commands: Vec<String>,

    /// Maximum number of model sampling steps in one turn.
    #[arg(long, default_value_t = 16)]
    max_steps: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let workspace = arguments
        .workspace
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", arguments.workspace.display()))?;
    let policy = if arguments.allowed_commands.is_empty() {
        WorkspacePolicy::new(&workspace)?
    } else {
        WorkspacePolicy::with_commands(&workspace, arguments.allowed_commands)?
    };
    let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY is required")?;
    let model = Arc::new(
        OpenAIResponsesClient::new(arguments.model, api_key, arguments.base_url)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?,
    );
    let tools = ToolRegistry::new(policy, CommandExecutor);
    let events = event_sink(arguments.json, arguments.events.as_deref())?;
    let agent = Agent::new(model, tools, events).with_max_steps(arguments.max_steps);
    let result = agent.run(&arguments.prompt).await?;

    if !arguments.json {
        println!("{}", result.final_message);
        print_git_diff(&workspace).await?;
    }
    Ok(())
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

async fn print_git_diff(workspace: &std::path::Path) -> Result<()> {
    let output = Command::new("git")
        .args(["diff", "--no-ext-diff"])
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    if let Ok(output) = output
        && output.status.success()
        && !output.stdout.is_empty()
    {
        println!("\nDiff:\n{}", String::from_utf8_lossy(&output.stdout));
    }
    Ok(())
}
