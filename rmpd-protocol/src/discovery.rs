use mdns_sd::{ServiceDaemon, ServiceEvent};
use rmpd_core::discovery::{DiscoveryCache, NetworkNeighbor};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Service types to discover on the network
const SERVICE_TYPES: &[&str] = &[
    "_mpd._tcp.local.",    // MPD servers
    "_smb._tcp.local.",    // Samba/SMB shares
    "_nfs._tcp.local.",    // NFS servers
    "_http._tcp.local.",   // HTTP/WebDAV servers
    "_webdav._tcp.local.", // WebDAV specific
];

/// Discovery service for finding network neighbors
pub struct DiscoveryService {
    cache: Arc<RwLock<DiscoveryCache>>,
    mdns: ServiceDaemon,
}

impl DiscoveryService {
    /// Create a new discovery service
    pub fn new() -> rmpd_core::error::Result<Arc<Self>> {
        let cache = Arc::new(RwLock::new(DiscoveryCache::new(Duration::from_secs(300))));
        let mdns = ServiceDaemon::new()?;

        Ok(Arc::new(Self { cache, mdns }))
    }

    /// Scan for network services and return discovered neighbors
    pub async fn scan_services(&self) -> rmpd_core::error::Result<Vec<NetworkNeighbor>> {
        // Check if cache is still valid
        {
            let cache = self.cache.read().await;
            if cache.is_valid() {
                debug!("returning cached discovery results");
                return Ok(cache.get().to_vec());
            }
        }

        info!("scanning for network services");
        let mut neighbors = Vec::new();

        // Browse each service type
        for service_type in SERVICE_TYPES {
            match self.browse_service(service_type).await {
                Ok(mut discovered) => {
                    debug!("found {} {} services", discovered.len(), service_type);
                    neighbors.append(&mut discovered);
                }
                Err(e) => {
                    warn!("failed to browse {}: {}", service_type, e);
                }
            }
        }

        // Update cache with results
        {
            let mut cache = self.cache.write().await;
            cache.update(neighbors.clone());
        }

        info!(
            "discovery scan complete, found {} neighbors",
            neighbors.len()
        );
        Ok(neighbors)
    }

    /// Browse a specific service type
    async fn browse_service(
        &self,
        service_type: &str,
    ) -> rmpd_core::error::Result<Vec<NetworkNeighbor>> {
        let receiver = self.mdns.browse(service_type)?;
        let mut neighbors = Vec::new();

        // Set a timeout for browsing
        let timeout = Duration::from_secs(3);
        let deadline = tokio::time::Instant::now() + timeout;

        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout_at(deadline, Self::recv_async(receiver.clone())).await {
                Ok(Ok(event)) => match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let protocol = Self::protocol_from_service_type(service_type);

                        // Get the best address (prefer IPv4)
                        if let Some(address) = info.get_addresses().iter().next() {
                            let addr_str = if info.get_port() > 0 {
                                format!("{}:{}", address, info.get_port())
                            } else {
                                address.to_string()
                            };

                            neighbors.push(NetworkNeighbor {
                                protocol: protocol.to_string(),
                                address: addr_str,
                                name: info.get_fullname().to_string(),
                            });

                            debug!(
                                "resolved {} service: {} at {}",
                                protocol,
                                info.get_fullname(),
                                address
                            );
                        }
                    }
                    ServiceEvent::SearchStarted(_) => {}
                    ServiceEvent::ServiceFound(_, _) => {
                        // Service found but not yet resolved, wait for ServiceResolved
                    }
                    ServiceEvent::ServiceRemoved(_, _) => {
                        // Service removed from network
                    }
                    _ => {}
                },
                Ok(Err(e)) => {
                    error!("error receiving mDNS event: {}", e);
                    break;
                }
                Err(_) => {
                    // Timeout - normal behavior
                    break;
                }
            }
        }

        Ok(neighbors)
    }

    /// Advertise this rmpd instance on the local network via mDNS.
    ///
    /// Registers a `_mpd._tcp.local.` service so MPD clients can auto-discover this server.
    pub fn advertise(&self, port: u16) -> rmpd_core::error::Result<()> {
        use mdns_sd::ServiceInfo;

        let hostname = std::fs::read_to_string("/etc/hostname")
            .unwrap_or_default()
            .trim()
            .to_string();
        let hostname = if hostname.is_empty() {
            "rmpd".to_string()
        } else {
            hostname
        };

        let instance_name = format!("rmpd@{}", hostname);
        let host_name = format!("{}.local.", hostname);

        let service_info = ServiceInfo::new(
            "_mpd._tcp.local.",
            &instance_name,
            &host_name,
            (),
            port,
            None,
        )?;

        self.mdns.register(service_info)?;
        info!("advertising rmpd as '{}' on port {}", instance_name, port);
        Ok(())
    }

    /// Convert async receiver to tokio-compatible async operation
    async fn recv_async(
        receiver: mdns_sd::Receiver<ServiceEvent>,
    ) -> Result<ServiceEvent, Box<dyn std::error::Error + Send + Sync>> {
        tokio::task::spawn_blocking(move || receiver.recv())
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Extract protocol name from mDNS service type
    fn protocol_from_service_type(service_type: &str) -> &str {
        if service_type.starts_with("_mpd.") {
            "mpd"
        } else if service_type.starts_with("_smb.") {
            "smb"
        } else if service_type.starts_with("_nfs.") {
            "nfs"
        } else if service_type.starts_with("_webdav.") {
            "webdav"
        } else if service_type.starts_with("_http.") {
            "http"
        } else {
            "unknown"
        }
    }

    /// Manually refresh the cache
    pub async fn refresh(&self) -> rmpd_core::error::Result<()> {
        let mut cache = self.cache.write().await;
        cache.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_from_service_type() {
        assert_eq!(
            DiscoveryService::protocol_from_service_type("_mpd._tcp.local."),
            "mpd"
        );
        assert_eq!(
            DiscoveryService::protocol_from_service_type("_smb._tcp.local."),
            "smb"
        );
        assert_eq!(
            DiscoveryService::protocol_from_service_type("_nfs._tcp.local."),
            "nfs"
        );
    }

    #[tokio::test]
    async fn test_discovery_service_creation() {
        let service = DiscoveryService::new();
        assert!(service.is_ok());
    }
}
