// Relay Registry - Tracks active relay nodes in the network
//
// This module maintains an in-memory registry of relay nodes that are currently
// serving the network. Nodes auto-register when they:
// 1. Have enableRelayServer = true
// 2. Are publicly reachable (AutoNAT reachability = Public)
// 3. Have at least one non-private listen address
//
// Registry is persisted in memory only for this sprint. Disk persistence can be
// added later if needed for faster bootstrap on node restart.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Information about a relay node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayInfo {
    /// Peer ID of the relay node
    pub peer_id: String,

    /// Multiaddrs where this relay can be reached
    pub addrs: Vec<String>,

    /// Optional friendly name/alias for this relay
    pub alias: Option<String>,

    /// Last time this relay was seen (unix timestamp seconds)
    pub last_seen: u64,

    /// Health score (0.0 - 1.0) based on relay metrics
    pub health_score: f32,
}

/// In-memory registry of active relay nodes
#[derive(Clone)]
pub struct RelayRegistry {
    /// Map of peer_id -> RelayInfo
    entries: Arc<RwLock<HashMap<String, RelayInfo>>>,
}

impl RelayRegistry {
    /// Create a new empty relay registry
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register or update a relay node in the registry
    ///
    /// This should be called:
    /// - When a node starts as a relay server
    /// - Periodically (at most once per 60 seconds) to refresh last_seen
    ///
    /// # Arguments
    /// * `peer_id` - The peer ID of the relay node
    /// * `addrs` - List of multiaddrs where the relay can be reached
    /// * `alias` - Optional friendly name for the relay
    /// * `health_score` - Health score (0.0 - 1.0) based on relay metrics
    pub async fn register(
        &self,
        peer_id: String,
        addrs: Vec<String>,
        alias: Option<String>,
        health_score: f32,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let relay_info = RelayInfo {
            peer_id: peer_id.clone(),
            addrs,
            alias: alias.clone(),
            last_seen: now,
            health_score: health_score.clamp(0.0, 1.0),
        };

        let mut entries = self.entries.write().await;
        let is_new = !entries.contains_key(&peer_id);
        entries.insert(peer_id.clone(), relay_info);

        if is_new {
            info!(
                "✅ Registered new relay: {} (alias: {:?}, health: {:.2})",
                peer_id,
                alias,
                health_score
            );
        } else {
            debug!(
                "🔄 Updated relay: {} (health: {:.2})",
                peer_id, health_score
            );
        }
    }

    /// Get all active relay nodes
    ///
    /// Returns a vector of all RelayInfo entries, sorted by health_score descending
    pub async fn list(&self) -> Vec<RelayInfo> {
        let entries = self.entries.read().await;
        let mut relays: Vec<RelayInfo> = entries.values().cloned().collect();

        // Sort by health score (best first)
        relays.sort_by(|a, b| {
            b.health_score
                .partial_cmp(&a.health_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        relays
    }

    /// Get a specific relay by peer_id
    pub async fn get(&self, peer_id: &str) -> Option<RelayInfo> {
        let entries = self.entries.read().await;
        entries.get(peer_id).cloned()
    }

    /// Remove stale relay entries
    ///
    /// Removes entries that haven't been seen for more than `max_age_secs` seconds
    ///
    /// # Arguments
    /// * `now` - Current unix timestamp in seconds
    /// * `max_age_secs` - Maximum age before an entry is considered stale
    ///
    /// # Returns
    /// Number of stale entries removed
    pub async fn prune_stale(&self, now: u64, max_age_secs: u64) -> usize {
        let mut entries = self.entries.write().await;
        let before_count = entries.len();

        entries.retain(|peer_id, relay| {
            let age = now.saturating_sub(relay.last_seen);
            let is_stale = age > max_age_secs;

            if is_stale {
                warn!(
                    "🗑️ Removing stale relay: {} (last seen {} seconds ago)",
                    peer_id, age
                );
            }

            !is_stale
        });

        let removed = before_count - entries.len();
        if removed > 0 {
            info!("🗑️ Pruned {} stale relay entries", removed);
        }

        removed
    }

    /// Get the total number of registered relays
    pub async fn count(&self) -> usize {
        let entries = self.entries.read().await;
        entries.len()
    }

    /// Check if a specific peer is registered as a relay
    pub async fn contains(&self, peer_id: &str) -> bool {
        let entries = self.entries.read().await;
        entries.contains_key(peer_id)
    }

    /// Remove a specific relay from the registry
    ///
    /// Returns true if the relay was found and removed
    pub async fn remove(&self, peer_id: &str) -> bool {
        let mut entries = self.entries.write().await;
        if entries.remove(peer_id).is_some() {
            info!("🗑️ Removed relay: {}", peer_id);
            true
        } else {
            false
        }
    }

    /// Clear all relay entries (useful for testing)
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
        info!("🗑️ Cleared all relay entries");
    }
}

impl Default for RelayRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_list() {
        let registry = RelayRegistry::new();

        // Register a relay
        registry
            .register(
                "peer1".to_string(),
                vec!["/ip4/1.2.3.4/tcp/4001".to_string()],
                Some("relay1".to_string()),
                0.9,
            )
            .await;

        // Check it's in the list
        let relays = registry.list().await;
        assert_eq!(relays.len(), 1);
        assert_eq!(relays[0].peer_id, "peer1");
        assert_eq!(relays[0].alias, Some("relay1".to_string()));
        assert_eq!(relays[0].health_score, 0.9);
    }

    #[tokio::test]
    async fn test_prune_stale() {
        let registry = RelayRegistry::new();

        // Register a relay with old timestamp
        let old_time = 1000;
        registry
            .register(
                "old_peer".to_string(),
                vec!["/ip4/1.2.3.4/tcp/4001".to_string()],
                None,
                0.8,
            )
            .await;

        // Manually set last_seen to old time
        {
            let mut entries = registry.entries.write().await;
            if let Some(relay) = entries.get_mut("old_peer") {
                relay.last_seen = old_time;
            }
        }

        // Register a recent relay
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        registry
            .register(
                "new_peer".to_string(),
                vec!["/ip4/5.6.7.8/tcp/4001".to_string()],
                None,
                0.9,
            )
            .await;

        // Prune stale entries (max age = 300 seconds)
        let removed = registry.prune_stale(now, 300).await;

        // Check that old entry was removed
        assert_eq!(removed, 1);
        assert_eq!(registry.count().await, 1);

        let relays = registry.list().await;
        assert_eq!(relays[0].peer_id, "new_peer");
    }

    #[tokio::test]
    async fn test_health_score_sorting() {
        let registry = RelayRegistry::new();

        // Register relays with different health scores
        registry
            .register(
                "peer1".to_string(),
                vec![],
                Some("low".to_string()),
                0.3,
            )
            .await;

        registry
            .register(
                "peer2".to_string(),
                vec![],
                Some("high".to_string()),
                0.9,
            )
            .await;

        registry
            .register(
                "peer3".to_string(),
                vec![],
                Some("medium".to_string()),
                0.6,
            )
            .await;

        // List should be sorted by health score (best first)
        let relays = registry.list().await;
        assert_eq!(relays.len(), 3);
        assert_eq!(relays[0].alias, Some("high".to_string()));
        assert_eq!(relays[1].alias, Some("medium".to_string()));
        assert_eq!(relays[2].alias, Some("low".to_string()));
    }

    #[tokio::test]
    async fn test_get_and_contains() {
        let registry = RelayRegistry::new();

        registry
            .register(
                "test_peer".to_string(),
                vec!["/ip4/1.2.3.4/tcp/4001".to_string()],
                Some("test".to_string()),
                0.8,
            )
            .await;

        // Test contains
        assert!(registry.contains("test_peer").await);
        assert!(!registry.contains("non_existent").await);

        // Test get
        let relay = registry.get("test_peer").await;
        assert!(relay.is_some());
        assert_eq!(relay.unwrap().alias, Some("test".to_string()));

        assert!(registry.get("non_existent").await.is_none());
    }

    #[tokio::test]
    async fn test_remove() {
        let registry = RelayRegistry::new();

        registry
            .register(
                "peer_to_remove".to_string(),
                vec![],
                None,
                0.8,
            )
            .await;

        assert_eq!(registry.count().await, 1);

        // Remove the relay
        let removed = registry.remove("peer_to_remove").await;
        assert!(removed);
        assert_eq!(registry.count().await, 0);

        // Try removing again
        let removed_again = registry.remove("peer_to_remove").await;
        assert!(!removed_again);
    }
}
