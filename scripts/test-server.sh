#!/bin/bash
# Start boringtun server locally with HTTP API + WebSocket proxy.
# Run with: sudo ./scripts/test-server.sh
#
# Prereqs:
#   1. Run ./scripts/gen-keys.sh first
#   2. cargo build --features payment

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
KEYS_DIR="$SCRIPT_DIR/keys"

if [ ! -f "$KEYS_DIR/server.key" ]; then
    echo "ERROR: Run ./scripts/gen-keys.sh first"
    exit 1
fi

SERVER_PRIVKEY=$(cat "$KEYS_DIR/server.key")
SERVER_PRIVKEY_HEX=$(echo -n "$SERVER_PRIVKEY" | base64 -d | xxd -p -c 64)

# Check for port conflicts
for PORT in 8089 8443; do
    if lsof -i :$PORT -P 2>/dev/null | grep -q LISTEN; then
        echo "ERROR: Port $PORT is already in use:"
        lsof -i :$PORT -P 2>/dev/null | grep LISTEN
        exit 1
    fi
done

# Kill any stale boringtun and clean up sockets
echo "[Cleanup] Killing stale boringtun processes..."
pkill -f "boringtun-cli utun" 2>/dev/null || true
sleep 1
rm -f /var/run/wireguard/utun*.sock 2>/dev/null || true

# Record existing sockets before we start
BEFORE_SOCKS=$(ls /var/run/wireguard/*.sock 2>/dev/null || true)

echo "=== Boringtun Server (local test) ==="
echo "Server public key: $(cat "$KEYS_DIR/server.pub")"
echo "HTTP API:          http://127.0.0.1:8089"
echo "WebSocket proxy:   ws://127.0.0.1:8443"
echo "WireGuard UDP:     127.0.0.1:51820"
echo "Subnet:            10.0.0.0/24 (server=10.0.0.1)"
echo ""

# Build if needed
BT_BIN="$ROOT_DIR/target/debug/boringtun-cli"
[ -f "$ROOT_DIR/target/release/boringtun-cli" ] && BT_BIN="$ROOT_DIR/target/release/boringtun-cli"

if [ ! -f "$BT_BIN" ]; then
    echo "[Build] Compiling boringtun..."
    cargo build --manifest-path "$ROOT_DIR/Cargo.toml" --features payment
fi

# Start boringtun in foreground
echo "[Start] Launching boringtun..."
BT_PAYMENT_SERVER=1 \
BT_HTTP_BIND="0.0.0.0:8089" \
BT_PUBLIC_IP="127.0.0.1" \
BT_WS_BIND="0.0.0.0:8443" \
BT_WG_PORT=51820 \
WG_LOG_LEVEL=info \
WG_SUDO=1 \
    "$BT_BIN" utun --foreground --disable-drop-privileges --disable-connected-udp &

BT_PID=$!
sleep 3

# Verify boringtun is still running
if ! kill -0 $BT_PID 2>/dev/null; then
    echo "ERROR: boringtun exited immediately. Check if you're running with sudo."
    exit 1
fi

# Find the NEW socket (not any stale ones)
UTUN_NAME=""
for sock in /var/run/wireguard/utun*.sock; do
    [ -e "$sock" ] || continue
    # Skip sockets that existed before we started
    if echo "$BEFORE_SOCKS" | grep -q "$sock"; then
        continue
    fi
    UTUN_NAME=$(basename "$sock" .sock)
    break
done

# Fallback: just use the newest socket
if [ -z "$UTUN_NAME" ]; then
    NEWEST_SOCK=$(ls -t /var/run/wireguard/utun*.sock 2>/dev/null | head -1)
    if [ -n "$NEWEST_SOCK" ]; then
        UTUN_NAME=$(basename "$NEWEST_SOCK" .sock)
    fi
fi

if [ -z "$UTUN_NAME" ]; then
    echo "ERROR: Could not find UAPI socket. Is boringtun running?"
    kill $BT_PID 2>/dev/null
    exit 1
fi

echo "[Setup] Interface: $UTUN_NAME"
echo "[Setup] UAPI socket: /var/run/wireguard/${UTUN_NAME}.sock"

# Configure private key and listen port via UAPI
echo "[Setup] Configuring WireGuard via UAPI..."
UAPI_RESULT=$(printf "set=1\nprivate_key=%s\nlisten_port=51820\n\n" "$SERVER_PRIVKEY_HEX" | \
    socat -t5 - UNIX-CONNECT:"/var/run/wireguard/${UTUN_NAME}.sock" 2>&1)
echo "[Setup] UAPI response: $UAPI_RESULT"

if echo "$UAPI_RESULT" | grep -q "errno=0"; then
    echo "[Setup] WireGuard configured successfully"
else
    echo "WARNING: UAPI config may have failed. Response: $UAPI_RESULT"
fi

# Assign IP to the interface (point-to-point)
ifconfig "$UTUN_NAME" 10.0.0.1 10.0.0.2 up 2>/dev/null || \
    echo "WARNING: Could not assign IP (need sudo?)"

# Route only assigned client IPs through the tunnel (NOT the whole /24).
# A blanket /24 route causes a kernel-level routing loop for non-existent IPs
# (packet goes to TUN → boringtun → back to TUN → repeat).
# The HTTP API adds /32 routes per peer on registration; for local testing
# we pre-add the first client IP.
route add -host 10.0.0.2 -interface "$UTUN_NAME" 2>/dev/null || true

# Enable IP forwarding and NAT for full tunnel support
echo "[Setup] Enabling IP forwarding and NAT..."
sysctl -w net.inet.ip.forwarding=1 >/dev/null 2>&1 || true

# Find the default internet interface (en0, en1, etc.)
INET_IF=$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')
if [ -n "$INET_IF" ]; then
    echo "[Setup] Internet interface: $INET_IF"
    # Create pf NAT rule: VPN clients → internet
    PF_CONF="/tmp/bt-pf-nat.conf"
    cat > "$PF_CONF" <<PFEOF
nat on $INET_IF from 10.0.0.0/24 to any -> ($INET_IF)
pass all
PFEOF
    pfctl -ef "$PF_CONF" 2>/dev/null || echo "WARNING: pfctl NAT setup failed"
    echo "[Setup] NAT enabled: 10.0.0.0/24 → $INET_IF"
else
    echo "WARNING: Could not detect internet interface for NAT"
fi

# Verify health endpoint
echo ""
echo "[Verify] Testing health endpoint..."
HEALTH=$(curl -s --connect-timeout 3 http://127.0.0.1:8089/health 2>&1) || true
if [ -n "$HEALTH" ]; then
    echo "[Verify] Health: $HEALTH"
else
    echo "[Verify] WARNING: Health endpoint not responding"
fi

echo ""
echo "=== Server running ==="
echo "To register a client peer:"
echo "  curl -X POST http://127.0.0.1:8089/v1/register -d '{\"public_key\":\"<client_pubkey_base64>\"}'"
echo ""
echo "Press Ctrl+C to stop"

# Cleanup on exit
cleanup() {
    echo ""
    echo "[Stop] Shutting down..."
    pfctl -d 2>/dev/null || true
    sysctl -w net.inet.ip.forwarding=0 >/dev/null 2>&1 || true
    kill $BT_PID 2>/dev/null
    wait $BT_PID 2>/dev/null
}
trap cleanup EXIT INT TERM

wait $BT_PID
