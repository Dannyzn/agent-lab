use agent_exec::{CommandExecutor, DEFAULT_MAX_OUTPUT_BYTES, ExecRequest};
use agent_policy::WorkspacePolicy;
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    #[serde(flatten)]
    pub metadata: serde_json::Map<String, Value>,
}

impl ToolResult {
    pub fn model_output(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    fn ok(output: String, metadata: Value) -> Self {
        Self {
            success: true,
            output,
            metadata: metadata.as_object().cloned().unwrap_or_default(),
        }
    }

    fn error(error: impl std::fmt::Display) -> Self {
        Self {
            success: false,
            output: error.to_string(),
            metadata: serde_json::Map::new(),
        }
    }
}

#[derive(Clone)]
pub struct ToolRegistry {
    policy: WorkspacePolicy,
    executor: CommandExecutor,
}

impl ToolRegistry {
    pub fn new(policy: WorkspacePolicy, executor: CommandExecutor) -> Self {
        Self { policy, executor }
    }

    pub fn specs(&self) -> Vec<Value> {
        vec![
            function_spec(
                "read_file",
                "Read a UTF-8 text file inside the workspace.",
                json!({"path":{"type":"string"}}),
                &["path"],
            ),
            function_spec(
                "search",
                "Search UTF-8 workspace files for a literal string.",
                json!({
                    "query":{"type":"string"},
                    "path":{"type":"string","default":"."}
                }),
                &["query"],
            ),
            function_spec(
                "apply_patch",
                "Apply a Begin Patch containing Update File sections.",
                json!({"patch":{"type":"string"}}),
                &["patch"],
            ),
            function_spec(
                "exec_command",
                "Run an allow-listed argv command without a shell.",
                json!({
                    "argv":{"type":"array","items":{"type":"string"}},
                    "timeout_ms":{"type":"integer","minimum":1,"maximum":120000}
                }),
                &["argv"],
            ),
        ]
    }

    pub async fn execute(&self, name: &str, arguments: &Value) -> ToolResult {
        let result = match name {
            "read_file" => self.read_file(arguments),
            "search" => self.search(arguments),
            "apply_patch" => self.apply_patch(arguments),
            "exec_command" => self.exec_command(arguments).await,
            _ => Err(anyhow!("unknown tool: {name}")),
        };
        result.unwrap_or_else(ToolResult::error)
    }

    fn read_file(&self, arguments: &Value) -> Result<ToolResult> {
        let requested = string_argument(arguments, "path")?;
        let path = self.policy.resolve_existing(requested)?;
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let relative = path.strip_prefix(self.policy.root())?.display().to_string();
        Ok(ToolResult::ok(content, json!({"path":relative})))
    }

    fn search(&self, arguments: &Value) -> Result<ToolResult> {
        let query = string_argument(arguments, "query")?;
        let requested = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        let root = self.policy.resolve_existing(requested)?;
        let mut files = Vec::new();
        collect_files(&root, self.policy.root(), &mut files)?;
        files.sort();
        let mut matches = Vec::new();
        for path in files {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            for (index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    let relative = path.strip_prefix(self.policy.root())?;
                    matches.push(format!("{}:{}:{}", relative.display(), index + 1, line));
                }
            }
        }
        let count = matches.len();
        Ok(ToolResult::ok(
            matches.join("\n"),
            json!({"match_count":count}),
        ))
    }

    fn apply_patch(&self, arguments: &Value) -> Result<ToolResult> {
        let patch = string_argument(arguments, "patch")?;
        let files = apply_update_patch(&self.policy, patch)?;
        Ok(ToolResult::ok(
            format!("updated {}", files.join(", ")),
            json!({"files":files}),
        ))
    }

    async fn exec_command(&self, arguments: &Value) -> Result<ToolResult> {
        let argv = arguments
            .get("argv")
            .and_then(Value::as_array)
            .context("argv must be an array")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .context("argv entries must be strings")
            })
            .collect::<Result<Vec<_>>>()?;
        self.policy.validate_command(&argv)?;
        let timeout_ms = arguments
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(30_000);
        if !(1..=120_000).contains(&timeout_ms) {
            bail!("timeout_ms must be between 1 and 120000");
        }
        let result = self
            .executor
            .run(ExecRequest {
                argv,
                cwd: self.policy.root().to_path_buf(),
                timeout_ms,
                max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            })
            .await?;
        Ok(ToolResult {
            success: result.success,
            output: result.output,
            metadata: json!({
                "exit_code":result.exit_code,
                "elapsed_ms":result.elapsed_ms,
                "timeout":result.timed_out,
                "truncated":result.truncated
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        })
    }
}

fn function_spec(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type":"function",
        "name":name,
        "description":description,
        "parameters":{
            "type":"object",
            "properties":properties,
            "required":required,
            "additionalProperties":false
        }
    })
}

fn string_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("{name} must be a string"))
}

fn collect_files(path: &Path, workspace: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        let resolved = path.canonicalize()?;
        if !resolved.starts_with(workspace) {
            return Ok(());
        }
        if resolved.is_file() {
            files.push(resolved);
        }
        return Ok(());
    }
    if metadata.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if path.file_name().is_some_and(|name| name == ".git") {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        collect_files(&entry?.path(), workspace, files)?;
    }
    Ok(())
}

pub fn apply_update_patch(policy: &WorkspacePolicy, patch: &str) -> Result<Vec<String>> {
    let lines: Vec<&str> = patch.lines().collect();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        bail!("patch must use Begin Patch / End Patch markers");
    }
    let mut index = 1;
    let mut changed = Vec::new();
    while index < lines.len() - 1 {
        let Some(relative) = lines[index].strip_prefix("*** Update File: ") else {
            bail!("only Update File patches are supported");
        };
        let path = policy.resolve_existing(relative)?;
        index += 1;
        let mut original = fs::read_to_string(&path)?;
        while index < lines.len() - 1 && lines[index].starts_with("@@") {
            index += 1;
            let mut old_lines = Vec::new();
            let mut new_lines = Vec::new();
            while index < lines.len() - 1
                && !lines[index].starts_with("@@")
                && !lines[index].starts_with("*** Update File: ")
            {
                let line = lines[index];
                let Some(marker) = line.chars().next() else {
                    bail!("invalid empty patch line");
                };
                if !matches!(marker, ' ' | '+' | '-') {
                    bail!("invalid patch line: {line}");
                }
                let content = &line[marker.len_utf8()..];
                if matches!(marker, ' ' | '-') {
                    old_lines.push(content);
                }
                if matches!(marker, ' ' | '+') {
                    new_lines.push(content);
                }
                index += 1;
            }
            let old = old_lines.join("\n");
            let new = new_lines.join("\n");
            if !original.contains(&old) {
                bail!("patch context not found in {relative}");
            }
            original = original.replacen(&old, &new, 1);
        }
        fs::write(&path, original)?;
        changed.push(relative.to_owned());
    }
    Ok(changed)
}
