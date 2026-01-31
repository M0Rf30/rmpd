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
    pub fn new() -> Result<Arc<Self>, anyhow::Error> {
        let cache = Arc::new(RwLock::new(DiscoveryCache::new(Duration::from_secs(300))));
        let mdns = ServiceDaemon::new()?;

        Ok(Arc::new(Self { cache, mdns }))
    }

    /// Scan for network services and return discovered neighbors
    pub async fn scan_services(&self) -> Result<Vec<NetworkNeighbor>, anyhow::Error> {
        // Check if cache is still valid
        {
            let cache = self.cache.read().await;
            if cache.is_valid() {
                debug!("Returning cached discovery results");
                return Ok(cache.get().to_vec());
            }
        }

        info!("Scanning for network services");
        let mut neighbors = Vec::new();

        // Browse each service type
        for service_type in SERVICE_TYPES {
            match self.browse_service(service_type).await {
                Ok(mut discovered) => {
                    debug!("Found {} {} services", discovered.len(), service_type);
                    neighbors.append(&mut discovered);
                }
                Err(e) => {
                    warn!("Failed to browse {}: {}", service_type, e);
                }
            }
        }

        // Update cache with results
        {
            let mut cache = self.cache.write().await;
            cache.update(neighbors.clone());
        }

        info!("Discovery scan complete, found {} neighbors", neighbors.len());
        Ok(neighbors)
    }

    /// Browse a specific service type
    async fn browse_service(&self, service_type: &str) -> Result<Vec<NetworkNeighbor>, anyhow::Error> {
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
                                "Resolved {} service: {} at {}",
                                protocol,
                                info.get_fullname(),
                                address
                            );
                        }
                    }
                    ServiceEvent::SearchStarted(_) => {
                        debug!("Search started for {}", service_type);
                    }
                    ServiceEvent::ServiceFound(_, _) => {
                        // Service found but not yet resolved, wait for ServiceResolved
                    }
                    ServiceEvent::ServiceRemoved(_, _) => {
                        // Service removed from network
                    }
                    _ => {}
                },
                Ok(Err(e)) => {
                    error!("Error receiving mDNS event: {}", e);
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
    pub async fn refresh(&self) -> Result<(), anyhow::Error> {
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
