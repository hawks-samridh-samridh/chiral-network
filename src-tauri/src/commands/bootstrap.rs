// Shared bootstrap node configuration
// This module provides bootstrap nodes for both Tauri commands and headless mode
//
// Bootstrap node override order:
// 1. User settings (customBootstrapNodes in app settings)
// 2. BOOTSTRAP_NODES environment variable (comma-separated multiaddrs)
// 3. bootstrap_nodes.json file (embedded in binary)
// 4. Hardcoded defaults (fallback)

use serde::{Deserialize, Serialize};
use tauri::command;

/// Bootstrap node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapNode {
    pub alias: String,
    pub multiaddr: String,
}

/// Container for the bootstrap nodes JSON file
#[derive(Debug, Deserialize)]
struct BootstrapNodesConfig {
    nodes: Vec<BootstrapNode>,
}

/// Get default bootstrap nodes with override logic
///
/// Override order:
/// 1. BOOTSTRAP_NODES env var (comma-separated multiaddrs)
/// 2. bootstrap_nodes.json (embedded in binary)
/// 3. Hardcoded defaults
pub fn get_bootstrap_nodes() -> Vec<String> {
    // 1. Check for BOOTSTRAP_NODES environment variable
    if let Ok(env_nodes) = std::env::var("BOOTSTRAP_NODES") {
        let nodes: Vec<String> = env_nodes
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if !nodes.is_empty() {
            tracing::info!("✅ Using bootstrap nodes from BOOTSTRAP_NODES env var: {} nodes", nodes.len());
            return nodes;
        }
    }

    // 2. Try to load from embedded bootstrap_nodes.json
    match load_bootstrap_nodes_from_json() {
        Ok(nodes) if !nodes.is_empty() => {
            tracing::info!("✅ Using bootstrap nodes from bootstrap_nodes.json: {} nodes", nodes.len());
            return nodes;
        }
        Ok(_) => tracing::warn!("⚠️ bootstrap_nodes.json is empty, falling back to hardcoded defaults"),
        Err(e) => tracing::warn!("⚠️ Failed to load bootstrap_nodes.json: {}, falling back to hardcoded defaults", e),
    }

    // 3. Fallback to hardcoded defaults
    tracing::info!("✅ Using hardcoded default bootstrap nodes");
    get_hardcoded_bootstrap_nodes()
}

/// Load bootstrap nodes from embedded JSON file
fn load_bootstrap_nodes_from_json() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Embed the JSON file at compile time
    let json_content = include_str!("../../bootstrap_nodes.json");
    let config: BootstrapNodesConfig = serde_json::from_str(json_content)?;

    Ok(config.nodes.iter().map(|node| node.multiaddr.clone()).collect())
}

/// Hardcoded bootstrap nodes (last resort fallback)
fn get_hardcoded_bootstrap_nodes() -> Vec<String> {
    vec![
        "/ip4/134.199.240.145/tcp/4001/p2p/12D3KooWFYTuQ2FY8tXRtFKfpXkTSipTF55mZkLntwtN1nHu83qE"
            .to_string(),
        "/ip4/136.116.190.115/tcp/4001/p2p/12D3KooWETLNJUVLbkAbenbSPPdwN9ZLkBU3TLfyAeEUW2dsVptr"
            .to_string(),
        "/ip4/130.245.173.105/tcp/4001/p2p/12D3KooWGFRvjXFBoU9y6xdteqP1kzctAXrYPoaDGmTGRHybZ6rp"
            .to_string(),
    ]
}

/// Get bootstrap nodes with full node info (alias + multiaddr)
///
/// This is useful for UI display and debugging
pub fn get_bootstrap_nodes_with_info() -> Vec<BootstrapNode> {
    // Try to load from JSON first to get aliases
    if let Ok(json_content) = std::env::var("BOOTSTRAP_NODES") {
        // If env var is set, we don't have aliases, so create simple ones
        let nodes: Vec<String> = json_content
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if !nodes.is_empty() {
            return nodes
                .into_iter()
                .enumerate()
                .map(|(i, multiaddr)| BootstrapNode {
                    alias: format!("bootstrap-{}", i + 1),
                    multiaddr,
                })
                .collect();
        }
    }

    // Try JSON file
    match load_bootstrap_nodes_config_from_json() {
        Ok(config) if !config.nodes.is_empty() => return config.nodes,
        _ => {}
    }

    // Fallback to hardcoded with default aliases
    vec![
        BootstrapNode {
            alias: "vincenzo-bootstrap".to_string(),
            multiaddr: "/ip4/134.199.240.145/tcp/4001/p2p/12D3KooWFYTuQ2FY8tXRtFKfpXkTSipTF55mZkLntwtN1nHu83qE".to_string(),
        },
        BootstrapNode {
            alias: "turtle-bootstrap-2".to_string(),
            multiaddr: "/ip4/136.116.190.115/tcp/4001/p2p/12D3KooWETLNJUVLbkAbenbSPPdwN9ZLkBU3TLfyAeEUW2dsVptr".to_string(),
        },
        BootstrapNode {
            alias: "whale-bootstrap-3".to_string(),
            multiaddr: "/ip4/130.245.173.105/tcp/4001/p2p/12D3KooWGFRvjXFBoU9y6xdteqP1kzctAXrYPoaDGmTGRHybZ6rp".to_string(),
        },
    ]
}

/// Load full bootstrap nodes config from embedded JSON
fn load_bootstrap_nodes_config_from_json() -> Result<BootstrapNodesConfig, Box<dyn std::error::Error>> {
    let json_content = include_str!("../../bootstrap_nodes.json");
    let config: BootstrapNodesConfig = serde_json::from_str(json_content)?;
    Ok(config)
}

/// Tauri command to get bootstrap nodes (returns just multiaddrs)
#[command]
pub fn get_bootstrap_nodes_command() -> Vec<String> {
    get_bootstrap_nodes()
}

/// Tauri command to get bootstrap nodes with full info (alias + multiaddr)
#[command]
pub fn get_bootstrap_nodes_with_info_command() -> Vec<BootstrapNode> {
    get_bootstrap_nodes_with_info()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardcoded_bootstrap_nodes() {
        let nodes = get_hardcoded_bootstrap_nodes();
        assert_eq!(nodes.len(), 3);
        assert!(nodes[0].contains("134.199.240.145"));
        assert!(nodes[1].contains("136.116.190.115"));
        assert!(nodes[2].contains("130.245.173.105"));
    }

    #[test]
    fn test_get_bootstrap_nodes_with_info_fallback() {
        // When no env var is set, should fall back to JSON or hardcoded defaults
        let nodes = get_bootstrap_nodes_with_info();
        assert!(!nodes.is_empty());

        // Check that all nodes have both alias and multiaddr
        for node in &nodes {
            assert!(!node.alias.is_empty());
            assert!(!node.multiaddr.is_empty());
            assert!(node.multiaddr.starts_with("/ip4/"));
        }
    }

    #[test]
    fn test_get_bootstrap_nodes_defaults() {
        // Should return at least the hardcoded defaults
        let nodes = get_bootstrap_nodes();
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn test_bootstrap_nodes_command() {
        // Command should return same result as function
        let nodes = get_bootstrap_nodes_command();
        assert!(!nodes.is_empty());
    }

    #[test]
    fn test_bootstrap_nodes_with_info_command() {
        // Command should return nodes with full info
        let nodes = get_bootstrap_nodes_with_info_command();
        assert!(!nodes.is_empty());

        for node in &nodes {
            assert!(!node.alias.is_empty());
            assert!(!node.multiaddr.is_empty());
        }
    }
}
