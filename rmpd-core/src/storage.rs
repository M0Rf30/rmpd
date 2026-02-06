pub mod platform;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

pub use platform::{get_default_backend, MountBackend};

/// Represents a mounted storage location
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MountPoint {
    /// Virtual path within music directory (e.g., "remote/server1")
    pub path: String,
    /// Source URI (e.g., "nfs://192.168.1.100/music", "smb://server/share")
    pub uri: String,
    /// Protocol type (e.g., "nfs", "smb", "http", "webdav")
    pub protocol: String,
    /// Whether the mount is currently active
    pub mounted: bool,
    /// Timestamp when the mount was established
    pub mounted_at: SystemTime,
}

impl MountPoint {
    /// Create a new mount point entry
    pub fn new(path: String, uri: String) -> Self {
        let protocol = Self::extract_protocol(&uri);

        Self {
            path,
            uri,
            protocol,
            mounted: false,
            mounted_at: SystemTime::now(),
        }
    }

    /// Extract protocol from URI
    fn extract_protocol(uri: &str) -> String {
        if let Some(pos) = uri.find("://") {
            uri[..pos].to_lowercase()
        } else {
            "unknown".to_string()
        }
    }
}

/// Registry for managing storage mounts
pub struct MountRegistry {
    mounts: Arc<RwLock<HashMap<String, MountPoint>>>,
}

impl MountRegistry {
    /// Create a new mount registry
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            mounts: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Register a new mount point (not yet mounted)
    pub async fn register(&self, path: String, uri: String) -> Result<(), String> {
        let mut mounts = self.mounts.write().await;

        if mounts.contains_key(&path) {
            return Err(format!("Mount point already exists: {path}"));
        }

        mounts.insert(path.clone(), MountPoint::new(path, uri));
        Ok(())
    }

    /// Register a mounted storage location
    pub async fn register_mounted(&self, path: String, uri: String) -> Result<(), String> {
        let mut mounts = self.mounts.write().await;

        let mut mount_point = MountPoint::new(path.clone(), uri);
        mount_point.mounted = true;
        mount_point.mounted_at = SystemTime::now();

        mounts.insert(path, mount_point);
        Ok(())
    }

    /// Unmount a storage location
    pub async fn unmount(&self, path: &str) -> Result<(), String> {
        let mut mounts = self.mounts.write().await;

        mounts
            .remove(path)
            .ok_or_else(|| format!("Mount point not found: {path}"))?;

        Ok(())
    }

    /// List all registered mounts
    pub async fn list(&self) -> Vec<MountPoint> {
        let mounts = self.mounts.read().await;
        mounts.values().cloned().collect()
    }

    /// Get a specific mount point
    pub async fn get(&self, path: &str) -> Option<MountPoint> {
        let mounts = self.mounts.read().await;
        mounts.get(path).cloned()
    }

    /// Check if a path is mounted
    pub async fn is_mounted(&self, path: &str) -> bool {
        let mounts = self.mounts.read().await;
        mounts.get(path).map(|m| m.mounted).unwrap_or(false)
    }

    /// Load mounts from serialized data
    pub async fn load(&self, data: HashMap<String, MountPoint>) {
        let mut mounts = self.mounts.write().await;
        *mounts = data;
    }

    /// Get all mounts as a HashMap for serialization
    pub async fn as_map(&self) -> HashMap<String, MountPoint> {
        let mounts = self.mounts.read().await;
        mounts.clone()
    }
}

impl Default for MountRegistry {
    fn default() -> Self {
        Self {
            mounts: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mount_registry_register() {
        let registry = MountRegistry::new();

        let result = registry
            .register(
                "remote/nas".to_string(),
                "nfs://192.168.1.100/music".to_string(),
            )
            .await;

        assert!(result.is_ok());

        let mounts = registry.list().await;
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].path, "remote/nas");
        assert_eq!(mounts[0].protocol, "nfs");
    }

    #[tokio::test]
    async fn test_mount_registry_duplicate() {
        let registry = MountRegistry::new();

        registry
            .register(
                "remote/nas".to_string(),
                "nfs://192.168.1.100/music".to_string(),
            )
            .await
            .unwrap();

        let result = registry
            .register(
                "remote/nas".to_string(),
                "nfs://192.168.1.200/music".to_string(),
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[tokio::test]
    async fn test_mount_registry_unmount() {
        let registry = MountRegistry::new();

        registry
            .register(
                "remote/nas".to_string(),
                "nfs://192.168.1.100/music".to_string(),
            )
            .await
            .unwrap();

        let result = registry.unmount("remote/nas").await;
        assert!(result.is_ok());

        let mounts = registry.list().await;
        assert_eq!(mounts.len(), 0);
    }

    #[tokio::test]
    async fn test_extract_protocol() {
        assert_eq!(MountPoint::extract_protocol("nfs://server/path"), "nfs");
        assert_eq!(MountPoint::extract_protocol("smb://server/share"), "smb");
        assert_eq!(MountPoint::extract_protocol("http://server:8080/"), "http");
        assert_eq!(MountPoint::extract_protocol("invalid"), "unknown");
    }

    #[tokio::test]
    async fn test_mount_status() {
        let registry = MountRegistry::new();

        registry
            .register(
                "remote/nas".to_string(),
                "nfs://192.168.1.100/music".to_string(),
            )
            .await
            .unwrap();

        assert!(!registry.is_mounted("remote/nas").await);

        registry
            .register_mounted(
                "remote/nas2".to_string(),
                "nfs://192.168.1.200/music".to_string(),
            )
            .await
            .unwrap();

        assert!(registry.is_mounted("remote/nas2").await);
    }
}
