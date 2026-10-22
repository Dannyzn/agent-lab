use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::path::{Component, Path, PathBuf};

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
}

impl WorkspacePolicy {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, PolicyError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| PolicyError(format!("cannot resolve workspace: {error}")))?;
        Ok(Self {
            root,
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

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_existing(&self, requested: &str) -> Result<PathBuf, PolicyError> {
        let relative = Path::new(requested);
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
