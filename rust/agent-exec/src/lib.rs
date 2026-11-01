use agent_protocol::CancellationToken;
use agent_sandbox::{SandboxConfig, TRUSTED_PATH, resolve_executable};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 65_536;

#[derive(Clone, Debug)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecResult {
    pub success: bool,
    pub output: String,
    pub exit_code: Option<i32>,
    pub elapsed_ms: u128,
    pub timed_out: bool,
    pub cancelled: bool,
    pub truncated: bool,
}

#[derive(Clone, Default)]
pub struct CommandExecutor {
    sandbox: SandboxConfig,
}

impl CommandExecutor {
    pub fn new(sandbox: SandboxConfig) -> Result<Self> {
        sandbox.validate()?;
        Ok(Self { sandbox })
    }

    pub fn sandbox(&self) -> &SandboxConfig {
        &self.sandbox
    }

    /// A read-only tool policy must never be paired with a writable executor.
    pub fn restrict_read_only(mut self) -> Self {
        self.sandbox.filesystem = agent_sandbox::FilesystemAccess::ReadOnly;
        self
    }

    pub async fn run(&self, request: ExecRequest) -> Result<ExecResult> {
        self.run_cancellable(request, &CancellationToken::default())
            .await
    }

    pub async fn run_cancellable(
        &self,
        request: ExecRequest,
        cancellation: &CancellationToken,
    ) -> Result<ExecResult> {
        anyhow::ensure!(
            !cancellation.is_cancelled(),
            "command cancelled before spawn"
        );
        let executable = resolve_executable(
            request
                .argv
                .first()
                .context("command argv cannot be empty")?,
        )?;
        let workspace = request.cwd.canonicalize()?;
        let scratch = tempfile::Builder::new().prefix("agent-exec-").tempdir()?;
        let scratch_path = scratch.path().canonicalize()?;
        let (program, prefix) = self
            .sandbox
            .launch(&executable, &workspace, &scratch_path)?;
        let started = Instant::now();
        let mut command = Command::new(program);
        command
            .args(prefix)
            .args(&request.argv[1..])
            .current_dir(&workspace)
            .env_clear()
            .env("PATH", TRUSTED_PATH)
            .env("HOME", &scratch_path)
            .env("TMPDIR", &scratch_path)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LANG", "en_US.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .context("failed to spawn sandboxed command")?;
        let mut group = ProcessGroup(Some(child.id().context("missing child pid")?));
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;
        // Drain continuously while retaining bounded prefixes.
        let stdout_reader = tokio::spawn(read_bounded(stdout, request.max_output_bytes));
        let stderr_reader = tokio::spawn(read_bounded(stderr, request.max_output_bytes));
        let mut timed_out = false;
        let mut cancelled = false;
        let status = tokio::select! {
            biased;
            _ = cancellation.cancelled() => { cancelled = true; None }
            _ = tokio::time::sleep(Duration::from_millis(request.timeout_ms)) => { timed_out = true; None }
            status = child.wait() => Some(status?),
        };
        // Kill descendants even after normal exit: they may retain pipe FDs.
        group.kill();
        if status.is_none() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let (mut combined, stdout_truncated) = collect_reader(stdout_reader).await?;
        let (stderr, stderr_truncated) = collect_reader(stderr_reader).await?;
        if !stderr.is_empty() {
            if !combined.is_empty() {
                combined.push(b'\n');
            }
            combined.extend(stderr);
        }
        let truncated =
            stdout_truncated || stderr_truncated || combined.len() > request.max_output_bytes;
        combined.truncate(request.max_output_bytes);
        let mut output = String::from_utf8_lossy(&combined).trim_end().to_owned();
        if truncated {
            output.push_str("\n...[output truncated]");
        }
        if timed_out {
            output.push_str("\ncommand timed out");
        }
        if cancelled {
            output.push_str("\ncommand cancelled");
        }
        Ok(ExecResult {
            success: status.is_some_and(|status| status.success()) && !cancelled && !timed_out,
            output,
            exit_code: status.and_then(|status| status.code()),
            elapsed_ms: started.elapsed().as_millis(),
            timed_out,
            cancelled,
            truncated,
        })
    }
}

async fn collect_reader(
    mut task: tokio::task::JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
) -> Result<(Vec<u8>, bool)> {
    match tokio::time::timeout(Duration::from_secs(1), &mut task).await {
        Ok(result) => Ok(result??),
        Err(_) => {
            task.abort();
            anyhow::bail!("descendant retained output pipe after process-group cleanup");
        }
    }
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut chunk = [0; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let retain = count.min(limit.saturating_sub(kept.len()));
        kept.extend_from_slice(&chunk[..retain]);
        truncated |= retain < count;
    }
    Ok((kept, truncated))
}

/// Own the process group across await points and dropped futures.
struct ProcessGroup(Option<u32>);
impl ProcessGroup {
    fn kill(&mut self) {
        if let Some(pid) = self.0.take() {
            #[cfg(unix)]
            // SAFETY: the child is placed in its own group with pgid == pid at spawn.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(script: &str, cwd: PathBuf) -> ExecRequest {
        ExecRequest {
            argv: vec!["python3".into(), "-c".into(), script.into()],
            cwd,
            timeout_ms: 5000,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    #[tokio::test]
    async fn enforces_timeout_and_bounded_output() {
        let root = tempfile::tempdir().unwrap();
        let mut req = request("import time; time.sleep(5)", root.path().to_path_buf());
        req.timeout_ms = 200;
        let result = CommandExecutor::default().run(req).await.unwrap();
        assert!(result.timed_out, "{result:?}");
        let mut req = request("print('abcdefghij' * 100000)", root.path().to_path_buf());
        req.max_output_bytes = 5;
        let result = CommandExecutor::default().run(req).await.unwrap();
        assert!(result.success, "{result:?}");
        assert!(result.truncated);
        assert!(result.output.starts_with("abcde"));
    }

    #[tokio::test]
    async fn cancellation_kills_descendants() {
        let root = tempfile::tempdir().unwrap();
        let script = "import subprocess,time,pathlib; subprocess.Popen(['/usr/bin/python3','-c',\"import time,pathlib; time.sleep(1); pathlib.Path('leaked').write_text('bad')\"]); pathlib.Path('ready').write_text('yes'); time.sleep(10)";
        let cancellation = CancellationToken::default();
        let task = tokio::spawn({
            let token = cancellation.clone();
            let req = request(script, root.path().to_path_buf());
            async move {
                CommandExecutor::default()
                    .run_cancellable(req, &token)
                    .await
                    .unwrap()
            }
        });
        for _ in 0..200 {
            if root.path().join("ready").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(root.path().join("ready").exists());
        cancellation.cancel();
        assert!(task.await.unwrap().cancelled);
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(!root.path().join("leaked").exists());
    }

    #[tokio::test]
    async fn timeout_and_normal_parent_exit_clean_descendants() {
        for timeout in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let script = format!(
                "import subprocess,time; subprocess.Popen(['/usr/bin/python3','-c',\"import time,pathlib; time.sleep(1); pathlib.Path('leaked').write_text('bad')\"]); {}",
                if timeout {
                    "time.sleep(10)"
                } else {
                    "print('done')"
                }
            );
            let mut req = request(&script, root.path().into());
            req.timeout_ms = 300;
            let result = CommandExecutor::default().run(req).await.unwrap();
            assert_eq!(result.timed_out, timeout, "{result:?}");
            assert_eq!(result.success, !timeout, "{result:?}");
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert!(!root.path().join("leaked").exists());
        }
    }

    #[tokio::test]
    async fn dropped_execution_future_kills_process_group() {
        let root = tempfile::tempdir().unwrap();
        let script = "import subprocess,time,pathlib; subprocess.Popen(['/usr/bin/python3','-c',\"import time,pathlib; time.sleep(1); pathlib.Path('leaked').write_text('bad')\"]); pathlib.Path('ready').write_text('yes'); time.sleep(10)";
        let req = request(script, root.path().into());
        let task = tokio::spawn(async move { CommandExecutor::default().run(req).await });
        for _ in 0..200 {
            if root.path().join("ready").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(root.path().join("ready").exists());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(!root.path().join("leaked").exists());
    }

    #[tokio::test]
    async fn seatbelt_enforces_filesystem_and_sanitizes_environment() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "test fixture only").unwrap();
        let script = format!(
            "import pathlib,os; assert 'OPENAI_API_KEY' not in os.environ; pathlib.Path('inside').write_text('ok'); pathlib.Path({:?}).read_text()",
            secret.to_str().unwrap()
        );
        let executor = CommandExecutor::default();
        let result = executor
            .run(request(&script, root.path().into()))
            .await
            .unwrap();
        assert!(!result.success, "{result:?}");
        assert!(result.output.contains("PermissionError"), "{result:?}");
        assert!(root.path().join("inside").exists());
        let write = format!(
            "from pathlib import Path; Path({:?}).write_text('bad')",
            secret.to_str().unwrap()
        );
        assert!(
            !executor
                .run(request(&write, root.path().into()))
                .await
                .unwrap()
                .success
        );
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "test fixture only"
        );
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let result = executor
            .run(request(
                "from pathlib import Path; Path('.git/config').write_text('bad')",
                root.path().into(),
            ))
            .await
            .unwrap();
        assert!(!result.success, "{result:?}");
        assert!(!root.path().join(".git/config").exists());
        let readonly = CommandExecutor::new(SandboxConfig {
            filesystem: agent_sandbox::FilesystemAccess::ReadOnly,
            ..Default::default()
        })
        .unwrap();
        let result = readonly
            .run(request(
                "from pathlib import Path; Path('forbidden').write_text('bad')",
                root.path().into(),
            ))
            .await
            .unwrap();
        assert!(!result.success, "{result:?}");
        assert!(!root.path().join("forbidden").exists());
    }

    #[tokio::test]
    async fn seatbelt_network_capability_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let script = format!(
            "import socket; socket.create_connection(('127.0.0.1', {}),timeout=1).close(); print('connected')",
            listener.local_addr().unwrap().port()
        );
        let denied = CommandExecutor::default()
            .run(request(&script, root.path().into()))
            .await
            .unwrap();
        assert!(!denied.success, "{denied:?}");
        let executor = CommandExecutor::new(SandboxConfig {
            network: agent_sandbox::NetworkAccess::Allow,
            ..Default::default()
        })
        .unwrap();
        let allowed = executor
            .run(request(&script, root.path().into()))
            .await
            .unwrap();
        assert!(allowed.success, "{allowed:?}");
        assert!(allowed.output.contains("connected"));
    }
}
