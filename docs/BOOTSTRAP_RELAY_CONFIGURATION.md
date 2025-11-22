# Bootstrap and Relay Configuration Guide

This document describes how to configure bootstrap nodes and relay servers in Chiral Network, including override precedence and the relay registry system.

## Bootstrap Nodes Configuration

### Overview

Bootstrap nodes are the initial peers that new nodes connect to when joining the network. They help new nodes discover other peers and integrate into the DHT.

### Configuration Sources

Bootstrap nodes are determined by the following precedence order (highest to lowest):

1. **User Settings** - Custom bootstrap nodes configured in Network Settings (`customBootstrapNodes`)
2. **Headless CLI Flag** - `--bootstrap` flag when running in headless mode
3. **Environment Variable** - `BOOTSTRAP_NODES` (comma-separated multiaddrs)
4. **JSON Configuration File** - `src-tauri/bootstrap_nodes.json` (embedded in binary)
5. **Hardcoded Defaults** - Fallback defaults in `src-tauri/src/commands/bootstrap.rs`

### Default Bootstrap Nodes

The default bootstrap nodes are defined in `src-tauri/bootstrap_nodes.json`:

```json
{
  "nodes": [
    {
      "alias": "vincenzo-bootstrap",
      "multiaddr": "/ip4/134.199.240.145/tcp/4001/p2p/12D3KooWFYTuQ2FY8tXRtFKfpXkTSipTF55mZkLntwtN1nHu83qE"
    },
    {
      "alias": "turtle-bootstrap-2",
      "multiaddr": "/ip4/136.116.190.115/tcp/4001/p2p/12D3KooWETLNJUVLbkAbenbSPPdwN9ZLkBU3TLfyAeEUW2dsVptr"
    },
    {
      "alias": "whale-bootstrap-3",
      "multiaddr": "/ip4/130.245.173.105/tcp/4001/p2p/12D3KooWGFRvjXFBoU9y6xdteqP1kzctAXrYPoaDGmTGRHybZ6rp"
    }
  ]
}
```

### Configuration Methods

#### 1. GUI Settings

Navigate to **Network Settings** and add custom bootstrap nodes in the "Custom Bootstrap Nodes" field.

#### 2. Headless Mode

```bash
# Single bootstrap node
chiral-network --bootstrap "/ip4/1.2.3.4/tcp/4001/p2p/PEER_ID"

# Multiple bootstrap nodes
chiral-network \
  --bootstrap "/ip4/1.2.3.4/tcp/4001/p2p/PEER_ID_1" \
  --bootstrap "/ip4/5.6.7.8/tcp/4001/p2p/PEER_ID_2"
```

#### 3. Environment Variable

```bash
export BOOTSTRAP_NODES="/ip4/1.2.3.4/tcp/4001/p2p/PEER_ID_1,/ip4/5.6.7.8/tcp/4001/p2p/PEER_ID_2"
chiral-network
```

#### 4. Modifying JSON File

Edit `src-tauri/bootstrap_nodes.json` before building the application:

```json
{
  "nodes": [
    {
      "alias": "my-custom-bootstrap",
      "multiaddr": "/ip4/YOUR_IP/tcp/4001/p2p/YOUR_PEER_ID"
    }
  ]
}
```

Then rebuild:
```bash
npm run tauri:build
```

## Relay Registry System

### Overview

The relay registry provides decentralized discovery of active relay servers in the network. It allows nodes to find available relays for NAT traversal and circuit relay functionality.

### Relay Discovery Architecture

Chiral Network uses **Kademlia DHT** as the canonical source of truth for relay discovery:

- **DHT Key**: `/chiral/relay-nodes/1.0.0` (libp2p-style namespace)
- **Storage**: All relay information is stored as a distributed key-value record in the Kademlia DHT
- **Local Cache**: `RelayRegistry` serves as an in-memory cache/view over the DHT data for fast access
- **HTTP Endpoint**: `/api/relay/registry` syncs from DHT before serving relay list

**Why DHT instead of libp2p rendezvous or AutoRelay?**

1. **rust-libp2p does NOT have an official AutoRelay implementation** (unlike go-libp2p)
2. **Kademlia DHT is fully decentralized** - any node can read/write without designated servers
3. **Rendezvous requires designated servers** - more centralized and requires infrastructure
4. **Community pattern** - rust-libp2p developers implement their own relay discovery using similar approaches

This design ensures relay discovery works without any centralized infrastructure while leveraging the battle-tested Kademlia DHT already used for file metadata.

### Auto-Registration

Nodes automatically register as relay servers when **all** of the following conditions are met:

1. **Relay Server Mode Enabled**: `enableRelayServer` is `true` in settings or `--enable-relay-server` flag in headless mode
2. **Public Reachability**: AutoNAT has determined the node is publicly reachable
3. **Public Listen Address**: The node has at least one non-private, non-localhost listen address
4. **Registry Available**: A relay registry instance is configured (typically on bootstrap nodes)

### Registration Frequency

- Nodes re-register **at most once per 60 seconds**
- Registration happens during DHT health polling (automatic)
- Health score is updated with each registration

### Relay Health Score

The health score (0.0 - 1.0) is calculated based on:
- Relay reservation success rate
- Connection uptime
- Number of active relay connections
- DCUtR hole-punching success rate

### Accessing the Relay Registry

The relay registry is exposed via HTTP on bootstrap nodes:

```bash
# Query the relay registry
curl http://BOOTSTRAP_IP:8545/api/relay/registry

# Example response
[
  {
    "peerId": "12D3KooW...",
    "addrs": ["/ip4/1.2.3.4/tcp/4001"],
    "alias": "my-relay-server",
    "lastSeen": 1699876543,
    "healthScore": 0.95
  }
]
```

### Bootstrap HTTP URL Configuration

The frontend queries the relay registry from the bootstrap server. Configure the URL:

#### GUI Settings

Navigate to **Network Settings** and set "Bootstrap HTTP URL" (default: `http://134.199.240.145:8545`)

#### Environment Variable

```bash
export CHIRAL_BOOTSTRAP_HTTP_URL="http://YOUR_BOOTSTRAP_IP:8545"
```

## Running as a Bootstrap Node

To run a node as a bootstrap server with relay registry:

```bash
# GUI mode
IS_BOOTSTRAP=1 chiral-network

# Headless mode
chiral-network \
  --is-bootstrap \
  --enable-relay-server \
  --relay-server-alias "my-bootstrap-relay" \
  --dht-port 4001
```

**Important**: Ensure ports 4001 (P2P) and 8545 (HTTP) are exposed and reachable.

## Network Page UI

The Network page displays bootstrap and relay status in a dedicated card:

- **Bootstrap Nodes Section**: Shows configured bootstrap nodes with their aliases
- **Relay Registry Section**: Displays active relays, their health scores, and last seen time
- **Refresh Button**: Manually refresh bootstrap and relay data

## Troubleshooting

### Bootstrap Nodes Not Connecting

1. Verify the multiaddr format: `/ip4/IP/tcp/PORT/p2p/PEER_ID`
2. Check network connectivity to the bootstrap node
3. Ensure the bootstrap node is running and reachable
4. Check logs for connection errors

### Relay Not Auto-Registering

1. Confirm `enableRelayServer` is `true`
2. Verify AutoNAT reports `Public` reachability (check Network page)
3. Ensure the node has public listen addresses (not localhost or private IPs)
4. Check that ports are properly exposed (no firewall blocking)

### Empty Relay Registry

1. Verify the bootstrap HTTP URL is correct
2. Check that the bootstrap node is running with relay registry enabled
3. Ensure at least one node in the network is publicly reachable and has relay server enabled
4. Check browser console for HTTP errors

## Technical Details

### Code Locations

- **Bootstrap Logic**: `src-tauri/src/commands/bootstrap.rs`
- **Relay Registry**: `src-tauri/src/relay_registry.rs`
- **Auto-Registration**: `src-tauri/src/dht.rs` (`maybe_register_as_relay()`)
- **HTTP Endpoint**: `src-tauri/src/http_server.rs` (`/api/relay/registry`)
- **Frontend Service**: `src/lib/dht.ts` (`getBootstrapNodesWithInfo()`, `getRelayRegistry()`)
- **UI Component**: `src/pages/Network.svelte` (Bootstrap and Relay Status card)

### Relay Registry Pruning

Stale entries (not seen for 5 minutes) are automatically removed when:
- The `/api/relay/registry` endpoint is queried
- The registry is accessed by the system

### Persistence

- **Bootstrap Configuration**: Embedded in binary (JSON) + runtime overrides
- **Relay Registry**: In-memory only (repopulated from live nodes on restart)

---

**Note**: For production deployments, ensure bootstrap nodes are highly available and geographically distributed for best network performance.
