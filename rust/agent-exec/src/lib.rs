use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;

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
    pub truncated: bool,
}

#[derive(Clone, Default)]
pub struct CommandExecutor;

impl CommandExecutor {
    pub async fn run(&self, request: ExecRequest) -> Result<ExecResult> {
        let executable = request
            .argv
            .first()
            .context("command argv cannot be empty")?;
        let started = Instant::now();
        let mut command = Command::new(executable);
        command
            .args(&request.argv[1..])
            .current_dir(&request.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .with_context(|| format!("failed to spawn {executable}"))?;
        match timeout(
            Duration::from_millis(request.timeout_ms),
            child.wait_with_output(),
        )
        .await
        {
            Ok(output) => {
                let output = output.context("failed to collect command output")?;
                let mut combined = output.stdout;
                if !output.stderr.is_empty() {
                    if !combined.is_empty() {
                        combined.push(b'\n');
                    }
                    combined.extend(output.stderr);
                }
                let (output_text, truncated) = truncate_utf8(combined, request.max_output_bytes);
                Ok(ExecResult {
                    success: output.status.success(),
                    output: output_text,
                    exit_code: output.status.code(),
                    elapsed_ms: started.elapsed().as_millis(),
                    timed_out: false,
                    truncated,
                })
            }
            Err(_) => Ok(ExecResult {
                success: false,
                output: "command timed out".into(),
                exit_code: None,
                elapsed_ms: started.elapsed().as_millis(),
                timed_out: true,
                truncated: false,
            }),
        }
    }
}

fn truncate_utf8(bytes: Vec<u8>, limit: usize) -> (String, bool) {
    if bytes.len() <= limit {
        return (String::from_utf8_lossy(&bytes).trim_end().to_owned(), false);
    }
    let prefix = String::from_utf8_lossy(&bytes[..limit]).into_owned();
    (
        format!("{}\n...[output truncated]", prefix.trim_end()),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enforces_timeout() {
        let result = CommandExecutor
            .run(ExecRequest {
                argv: vec![
                    "python3".into(),
                    "-c".into(),
                    "import time; time.sleep(1)".into(),
                ],
                cwd: std::env::current_dir().unwrap(),
                timeout_ms: 10,
                max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            })
            .await
            .unwrap();
        assert!(result.timed_out);
        assert!(!result.success);
    }

    #[tokio::test]
    async fn truncates_combined_output() {
        let result = CommandExecutor
            .run(ExecRequest {
                argv: vec!["python3".into(), "-c".into(), "print('abcdefghij')".into()],
                cwd: std::env::current_dir().unwrap(),
                timeout_ms: 10_000,
                max_output_bytes: 5,
            })
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.truncated);
        assert!(result.output.starts_with("abcde"));
    }
}
