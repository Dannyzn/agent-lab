use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Default)]
pub enum ApprovalMode {
    #[default]
    Ask,
    Never,
    Auto,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Ask(String),
    Deny(String),
}

#[derive(Debug)]
pub struct PolicyError(pub String);

impl Display for PolicyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PolicyError {}

#[derive(Clone, Debug)]
pub struct WorkspacePolicy {
    root: PathBuf,
    allowed_commands: HashSet<String>,
    read_only: bool,
}

impl WorkspacePolicy {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, PolicyError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| PolicyError(format!("cannot resolve workspace: {error}")))?;
        if !root.is_dir() {
            return Err(PolicyError("workspace must be a directory".into()));
        }
        Ok(Self {
            root,
            read_only: false,
            allowed_commands: ["python", "python3", "pytest", "git", "rg", "ls", "cat"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        })
    }

    pub fn with_commands(
        root: impl AsRef<Path>,
        commands: impl IntoIterator<Item = String>,
    ) -> Result<Self, PolicyError> {
        let mut policy = Self::new(root)?;
        policy.allowed_commands = commands.into_iter().collect();
        Ok(policy)
    }

    pub fn allowed_commands(&self) -> Vec<&str> {
        let mut commands: Vec<_> = self.allowed_commands.iter().map(String::as_str).collect();
        commands.sort_unstable();
        commands
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn decide(
        &self,
        name: &str,
        arguments: &serde_json::Value,
        mode: ApprovalMode,
    ) -> PolicyDecision {
        if !matches!(
            name,
            "read_file" | "search" | "apply_patch" | "exec_command"
        ) {
            return PolicyDecision::Deny(format!("unknown tool: {name}"));
        }
        if name == "apply_patch" && self.read_only {
            return PolicyDecision::Deny("workspace is read-only".into());
        }
        if name == "exec_command" {
            let argv: Result<Vec<String>, _> = serde_json::from_value(arguments["argv"].clone());
            let error = match argv {
                Ok(argv) => self.validate_command(&argv).err().map(|e| e.to_string()),
                Err(_) => Some("argv must be an array of strings".into()),
            };
            if let Some(error) = error {
                return PolicyDecision::Deny(error);
            }
        }
        if matches!(name, "read_file" | "search") {
            return PolicyDecision::Allow;
        }
        match mode {
            ApprovalMode::Auto => PolicyDecision::Allow,
            ApprovalMode::Never => {
                PolicyDecision::Deny("execution requires approval; approval mode is never".into())
            }
            ApprovalMode::Ask => {
                PolicyDecision::Ask(format!("{name} may execute code or modify workspace files"))
            }
        }
    }

    pub fn resolve_existing(&self, requested: &str) -> Result<PathBuf, PolicyError> {
        let relative = Path::new(requested);
        if relative.components().any(|part| part.as_os_str() == ".git") {
            return Err(PolicyError("Git metadata is reserved to the host".into()));
        }
        if relative.is_absolute() {
            return Err(PolicyError(format!(
                "absolute path is not allowed: {requested}"
            )));
        }
        if relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(PolicyError(format!(
                "parent path components are not allowed: {requested}"
            )));
        }
        let candidate = self.root.join(relative).canonicalize().map_err(|error| {
            PolicyError(format!(
                "cannot resolve workspace path {requested}: {error}"
            ))
        })?;
        if !candidate.starts_with(&self.root) {
            return Err(PolicyError(format!("path escapes workspace: {requested}")));
        }
        if candidate
            .strip_prefix(&self.root)
            .is_ok_and(|relative| relative.components().any(|part| part.as_os_str() == ".git"))
        {
            return Err(PolicyError("Git metadata is reserved to the host".into()));
        }
        Ok(candidate)
    }

    pub fn validate_command(&self, argv: &[String]) -> Result<(), PolicyError> {
        let executable = argv
            .first()
            .ok_or_else(|| PolicyError("command argv cannot be empty".into()))?;
        let path = Path::new(executable);
        if path.components().count() != 1
            || !matches!(path.components().next(), Some(Component::Normal(_)))
        {
            return Err(PolicyError(
                "command must use an allow-listed bare executable name".into(),
            ));
        }
        if !self.allowed_commands.contains(executable) {
            return Err(PolicyError(format!("command is not allowed: {executable}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_bare_and_unlisted_commands() {
        let root = std::env::current_dir().unwrap();
        let policy = WorkspacePolicy::new(root).unwrap();
        assert!(policy.validate_command(&["sh".into()]).is_err());
        assert!(policy.validate_command(&["/tmp/python3".into()]).is_err());
        assert!(policy.validate_command(&["python3".into()]).is_ok());
        assert!(policy.resolve_existing("../outside").is_err());
    }
}
