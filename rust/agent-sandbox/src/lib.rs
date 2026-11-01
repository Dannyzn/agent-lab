use anyhow::{Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const TRUSTED_PATH: &str = "/Library/Developer/CommandLineTools/usr/bin:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin";

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemAccess {
    ReadOnly,
    #[default]
    WorkspaceWrite,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccess {
    #[default]
    Deny,
    Allow,
}

#[derive(Clone, Debug, Serialize)]
pub struct SandboxConfig {
    pub enabled: bool,
    pub filesystem: FilesystemAccess,
    pub network: NetworkAccess,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            filesystem: FilesystemAccess::WorkspaceWrite,
            network: NetworkAccess::Deny,
        }
    }
}

impl SandboxConfig {
    pub fn validate(&self) -> Result<()> {
        if self.enabled
            && (!cfg!(target_os = "macos") || !Path::new("/usr/bin/sandbox-exec").is_file())
        {
            bail!(
                "OS sandbox unavailable: Phase 2 supports macOS Seatbelt; refusing unsandboxed execution"
            );
        }
        Ok(())
    }

    /// Dynamic paths are parameters, never interpolated into Scheme source.
    pub fn launch(
        &self,
        executable: &Path,
        workspace: &Path,
        scratch: &Path,
    ) -> Result<(PathBuf, Vec<String>)> {
        self.validate()?;
        if !self.enabled {
            return Ok((executable.to_path_buf(), Vec::new()));
        }
        let mut args = vec!["-p".into(), self.profile()];
        for (name, path) in [("WORKSPACE", workspace), ("SCRATCH", scratch)] {
            args.push("-D".into());
            args.push(format!(
                "{name}={}",
                path.to_str()
                    .ok_or_else(|| anyhow::anyhow!("sandbox paths must be UTF-8"))?
            ));
        }
        args.push(executable.display().to_string());
        Ok((PathBuf::from("/usr/bin/sandbox-exec"), args))
    }

    pub fn profile(&self) -> String {
        let mut profile = r#"(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow system-mac-syscall (mac-policy-name "vnguard"))
(allow system-mac-syscall (require-all (mac-policy-name "Sandbox") (mac-syscall-number 67)))
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo"))
(allow file-read-metadata)
(allow file-read* (literal "/"))
(allow file-read* file-map-executable
  (subpath "/System") (subpath "/usr") (subpath "/bin") (subpath "/sbin")
  (subpath "/Library/Developer") (subpath "/Library/Frameworks")
  (subpath "/Library/Apple") (subpath "/opt/homebrew")
  (subpath "/private/var/db/timezone")
  (literal "/private/etc/localtime") (literal "/private/etc/passwd")
  (literal "/dev/random") (literal "/dev/urandom")
  (subpath (param "WORKSPACE")))
(allow file-read* file-write* (subpath (param "SCRATCH")))
(allow file-read* file-write-data (literal "/dev/null") (subpath "/dev/fd"))
"#
        .to_owned();
        if self.filesystem == FilesystemAccess::WorkspaceWrite {
            profile.push_str("(allow file-write* (subpath (param \"WORKSPACE\")))\n");
        }
        // Protect Git metadata even when workspace writes are enabled.
        profile.push_str(
            "(deny file-write* (subpath (string-append (param \"WORKSPACE\") \"/.git\")))\n",
        );
        if self.network == NetworkAccess::Allow {
            profile.push_str("(allow network*)\n(allow mach-lookup (global-name \"com.apple.SystemConfiguration.configd\") (global-name \"com.apple.SystemConfiguration.DNSConfiguration\"))\n(allow file-read* (literal \"/private/etc/resolv.conf\") (literal \"/private/etc/hosts\"))\n");
        }
        profile
    }
}

pub fn resolve_executable(name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    if path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        bail!("executable must be a bare name");
    }
    for directory in TRUSTED_PATH.split(':') {
        let candidate = Path::new(directory).join(name);
        if candidate.is_file() {
            return Ok(candidate.canonicalize()?);
        }
    }
    bail!("executable not found in trusted PATH: {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_does_not_interpolate_workspace() {
        let sandbox = SandboxConfig::default();
        let root = tempfile::tempdir().unwrap();
        let (_, args) = sandbox
            .launch(Path::new("/usr/bin/true"), root.path(), root.path())
            .unwrap();
        assert!(!args[1].contains(root.path().to_str().unwrap()));
        assert!(!args[1].contains("(allow network*)"));
        assert!(args.iter().any(|arg| arg.starts_with("WORKSPACE=")));
    }
}
