# Local Test Scripts

Test boringtun server + client with WebSocket proxy on macOS.

## Prerequisites

```bash
brew install socat wireguard-tools
cargo build --features payment
```

## Quick Start

```bash
# Terminal 1: Generate keys
./scripts/gen-keys.sh

# Terminal 2: Start server (needs sudo for TUN)
sudo ./scripts/test-server.sh

# Terminal 3: Start client (needs sudo for TUN)
sudo ./scripts/test-client.sh

# Terminal 3: Test
ping 10.0.0.1
```

## What happens

```
┌─────────────┐     UDP      ┌────────────┐     WS       ┌────────────┐     UDP      ┌─────────────┐
│  boringtun   │ ──────────> │  ws-bridge  │ ──────────> │  ws_proxy   │ ──────────> │  boringtun   │
│   client     │ <────────── │   (node)    │ <────────── │  (server)   │ <────────── │   server     │
│  10.0.0.x    │   :51821    │             │   :8443     │             │  127.0.0.1  │  10.0.0.1    │
└─────────────┘              └────────────┘              └────────────┘   :51820     └─────────────┘
```

- **Server**: boringtun + HTTP API (:8080) + WS proxy (:8443)
- **Client**: boringtun + ws-bridge (local UDP :51821 → server WS :8443)
- Client registers via `POST /v1/register` to get assigned IP + server pubkey
- All WireGuard traffic flows through WebSocket (no direct UDP exposure)
