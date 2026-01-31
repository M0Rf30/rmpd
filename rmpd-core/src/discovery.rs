use std::time::{Duration, Instant};

/// Represents a discovered network service/neighbor
#[derive(Clone, Debug)]
pub struct NetworkNeighbor {
    /// Protocol type (e.g., "mpd", "smb", "nfs", "http")
    pub protocol: String,
    /// Network address (e.g., "192.168.1.100:6600")
    pub address: String,
    /// Friendly name of the service
    pub name: String,
}

/// Cache for discovered network neighbors
#[derive(Debug)]
pub struct DiscoveryCache {
    /// List of discovered neighbors
    neighbors: Vec<NetworkNeighbor>,
    /// Timestamp of last successful scan
    last_scan: Option<Instant>,
    /// Time-to-live for cached results
    ttl: Duration,
}

impl DiscoveryCache {
    /// Create a new discovery cache with specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            neighbors: Vec::new(),
            last_scan: None,
            ttl,
        }
    }

    /// Check if cache is still valid
    pub fn is_valid(&self) -> bool {
        self.last_scan
            .map(|last| last.elapsed() < self.ttl)
            .unwrap_or(false)
    }

    /// Update cache with new neighbors
    pub fn update(&mut self, neighbors: Vec<NetworkNeighbor>) {
        self.neighbors = neighbors;
        self.last_scan = Some(Instant::now());
    }

    /// Get cached neighbors
    pub fn get(&self) -> &[NetworkNeighbor] {
        &self.neighbors
    }

    /// Clear the cache
    pub fn clear(&mut self) {
        self.neighbors.clear();
        self.last_scan = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_validity() {
        let mut cache = DiscoveryCache::new(Duration::from_secs(300));
        assert!(!cache.is_valid());

        cache.update(vec![NetworkNeighbor {
            protocol: "mpd".to_string(),
            address: "192.168.1.100:6600".to_string(),
            name: "Test Server".to_string(),
        }]);

        assert!(cache.is_valid());
        assert_eq!(cache.get().len(), 1);
    }

    #[test]
    fn test_cache_expiration() {
        let mut cache = DiscoveryCache::new(Duration::from_millis(10));
        cache.update(vec![]);
        assert!(cache.is_valid());

        std::thread::sleep(Duration::from_millis(20));
        assert!(!cache.is_valid());
    }
}
