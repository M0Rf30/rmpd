use crate::error::{Result, RmpdError};
use std::path::Path;
use std::process::Command;

/// Parse URI into protocol and address (e.g. "nfs://server/path" -> ("nfs", "server/path"))
fn parse_uri(uri: &str) -> Result<(String, String)> {
    if let Some(pos) = uri.find("://") {
        let protocol = uri[..pos].to_lowercase();
        let address = &uri[pos + 3..];
        Ok((protocol, address.to_string()))
    } else {
        Err(RmpdError::Storage(format!("Invalid URI format: {uri}")))
    }
}

/// Convert a Path to a UTF-8 str, returning a descriptive error on failure
fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| RmpdError::Storage(format!("Invalid UTF-8 in path: {}", path.display())))
}

/// Platform-agnostic mount backend trait
pub trait MountBackend: Send + Sync {
    /// Mount a remote filesystem
    fn mount(&self, uri: &str, mountpoint: &Path, options: &[String]) -> Result<()>;

    /// Unmount a filesystem
    fn unmount(&self, mountpoint: &Path) -> Result<()>;

    /// Check if a path is currently mounted
    fn is_mounted(&self, mountpoint: &Path) -> bool;
}

/// Linux mount backend using system mount commands
#[cfg(target_os = "linux")]
pub struct LinuxMountBackend;

#[cfg(target_os = "linux")]
impl Default for LinuxMountBackend {
    fn default() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl LinuxMountBackend {
    pub fn new() -> Self {
        Self
    }

    /// Execute mount command and check result
    fn execute_mount_command(
        &self,
        fs_type: &str,
        source: &str,
        target: &str,
        options: &[String],
    ) -> Result<()> {
        let mut cmd = Command::new("mount");
        cmd.arg("-t").arg(fs_type).arg(source).arg(target);

        if !options.is_empty() {
            cmd.arg("-o").arg(options.join(","));
        }

        let output = cmd
            .output()
            .map_err(|e| RmpdError::Storage(format!("Failed to execute mount command: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RmpdError::Storage(format!("Mount failed: {stderr}")));
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl MountBackend for LinuxMountBackend {
    fn mount(&self, uri: &str, mountpoint: &Path, options: &[String]) -> Result<()> {
        let (protocol, address) = parse_uri(uri)?;

        match protocol.as_str() {
            "nfs" => {
                // NFS mount: mount -t nfs server:/path /mountpoint
                tracing::info!("Mounting NFS: {} -> {}", address, mountpoint.display());
                self.execute_mount_command("nfs", &address, path_to_str(mountpoint)?, options)
            }
            "smb" | "cifs" => {
                // SMB/CIFS mount: mount -t cifs //server/share /mountpoint
                let cifs_path = if address.starts_with("//") {
                    address.clone()
                } else {
                    format!("//{}", address)
                };

                tracing::info!("Mounting CIFS: {} -> {}", cifs_path, mountpoint.display());

                // Add guest option if no credentials provided
                let mut mount_options = options.to_vec();
                if !options.iter().any(|opt| opt.contains("username")) {
                    mount_options.push("guest".to_string());
                }

                self.execute_mount_command(
                    "cifs",
                    &cifs_path,
                    path_to_str(mountpoint)?,
                    &mount_options,
                )
            }
            "webdav" | "http" | "https" => {
                // WebDAV would require davfs2 to be installed
                // mount -t davfs http://server/path /mountpoint
                tracing::warn!("WebDAV mounting requires davfs2 to be installed and configured");

                // Try with davfs
                self.execute_mount_command("davfs", uri, path_to_str(mountpoint)?, options)
            }
            _ => Err(RmpdError::Storage(format!(
                "Unsupported protocol: {protocol}"
            ))),
        }
    }

    fn unmount(&self, mountpoint: &Path) -> Result<()> {
        tracing::info!("Unmounting: {}", mountpoint.display());

        let output = Command::new("umount")
            .arg(path_to_str(mountpoint)?)
            .output()
            .map_err(|e| RmpdError::Storage(format!("Failed to execute umount: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            // Check if it's because it's not mounted
            if stderr.contains("not mounted") {
                return Ok(()); // Already unmounted, treat as success
            }

            return Err(RmpdError::Storage(format!("Unmount failed: {stderr}")));
        }

        Ok(())
    }

    fn is_mounted(&self, mountpoint: &Path) -> bool {
        // Check /proc/mounts to see if path is mounted
        if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
            let mountpoint_str = mountpoint.to_string_lossy();
            mounts.lines().any(|line| {
                line.split_whitespace()
                    .nth(1)
                    .map(|mp| mp == mountpoint_str)
                    .unwrap_or(false)
            })
        } else {
            false
        }
    }
}

/// macOS mount backend using system mount commands
#[cfg(target_os = "macos")]
pub struct MacOSMountBackend;

#[cfg(target_os = "macos")]
impl MacOSMountBackend {
    pub fn new() -> Self {
        Self
    }

}

#[cfg(target_os = "macos")]
impl MountBackend for MacOSMountBackend {
    fn mount(&self, uri: &str, mountpoint: &Path, _options: &[String]) -> Result<()> {
        let (protocol, address) = parse_uri(uri)?;

        match protocol.as_str() {
            "nfs" => {
                // macOS NFS mount: mount -t nfs server:/path /mountpoint
                let output = Command::new("mount")
                    .arg("-t")
                    .arg("nfs")
                    .arg(&address)
                    .arg(path_to_str(mountpoint)?)
                    .output()
                    .map_err(|e| RmpdError::Storage(format!("Mount command failed: {e}")))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(RmpdError::Storage(format!("Mount failed: {stderr}")));
                }
                Ok(())
            }
            "smb" | "cifs" => {
                // macOS SMB mount: mount -t smbfs //server/share /mountpoint
                let smb_path = if address.starts_with("//") {
                    address.clone()
                } else {
                    format!("//{}", address)
                };

                let output = Command::new("mount")
                    .arg("-t")
                    .arg("smbfs")
                    .arg(&smb_path)
                    .arg(path_to_str(mountpoint)?)
                    .output()
                    .map_err(|e| RmpdError::Storage(format!("Mount command failed: {e}")))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(RmpdError::Storage(format!("Mount failed: {stderr}")));
                }
                Ok(())
            }
            _ => Err(RmpdError::Storage(format!(
                "Unsupported protocol: {protocol}"
            ))),
        }
    }

    fn unmount(&self, mountpoint: &Path) -> Result<()> {
        let output = Command::new("umount")
            .arg(path_to_str(mountpoint)?)
            .output()
            .map_err(|e| RmpdError::Storage(format!("Unmount command failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not currently mounted") {
                return Ok(());
            }
            return Err(RmpdError::Storage(format!("Unmount failed: {stderr}")));
        }

        Ok(())
    }

    fn is_mounted(&self, mountpoint: &Path) -> bool {
        // Use mount command to check if mounted
        if let Ok(output) = Command::new("mount").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mountpoint_str = mountpoint.to_string_lossy();
            stdout
                .lines()
                .any(|line| line.contains(&format!("on {} ", mountpoint_str)))
        } else {
            false
        }
    }
}

/// Stub backend for unsupported platforms
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub struct StubMountBackend;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl MountBackend for StubMountBackend {
    fn mount(&self, _uri: &str, _mountpoint: &Path, _options: &[String]) -> Result<()> {
        Err(RmpdError::Storage(
            "Mounting not supported on this platform".to_string(),
        ))
    }

    fn unmount(&self, _mountpoint: &Path) -> Result<()> {
        Err(RmpdError::Storage(
            "Unmounting not supported on this platform".to_string(),
        ))
    }

    fn is_mounted(&self, _mountpoint: &Path) -> bool {
        false
    }
}

/// Get the default mount backend for the current platform
pub fn get_default_backend() -> Box<dyn MountBackend> {
    #[cfg(target_os = "linux")]
    {
        Box::new(LinuxMountBackend::new())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(MacOSMountBackend::new())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Box::new(StubMountBackend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uri() {
        let (proto, addr) = parse_uri("nfs://192.168.1.100/music").unwrap();
        assert_eq!(proto, "nfs");
        assert_eq!(addr, "192.168.1.100/music");

        let (proto, addr) = parse_uri("smb://server/share").unwrap();
        assert_eq!(proto, "smb");
        assert_eq!(addr, "server/share");
    }

    #[test]
    fn test_parse_uri_invalid() {
        let result = parse_uri("invalid_uri");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_default_backend() {
        let backend = get_default_backend();
        // Just ensure it doesn't panic
        assert!(!backend.is_mounted(Path::new("/nonexistent")));
    }
}
