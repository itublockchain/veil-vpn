#!/bin/bash
# Start boringtun server on a Linux VPS.
# Usage: sudo ./scripts/test-server-linux.sh
#
# Prereqs:
#   1. Run ./scripts/gen-keys.sh first
#   2. cargo build --release -p boringtun-cli --features payment
#   3. socat installed (apt install socat)

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
KEYS_DIR="$SCRIPT_DIR/keys"

[ -f "$KEYS_DIR/server.key" ] || { echo "ERROR: Run ./scripts/gen-keys.sh first"; exit 1; }

SERVER_PRIVKEY_HEX=$(base64 -d "$KEYS_DIR/server.key" | xxd -p -c 64)
PUBLIC_IP="${PUBLIC_IP:-37.27.29.160}"

BT_BIN="$ROOT_DIR/target/release/boringtun-cli"
[ -f "$BT_BIN" ] || BT_BIN="$ROOT_DIR/target/debug/boringtun-cli"
[ -f "$BT_BIN" ] || { echo "ERROR: Build first: cargo build --release -p boringtun-cli --features payment"; exit 1; }

# Kill stale instances
pkill -f "boringtun.*wg0" 2>/dev/null || true
sleep 1
rm -f /var/run/wireguard/wg0.sock

echo "=== Veil VPN Server ==="
echo "Public IP:     $PUBLIC_IP"
echo "HTTP API:      http://$PUBLIC_IP:8089"
echo "WebSocket:     ws://$PUBLIC_IP:8443"
echo "WireGuard:     $PUBLIC_IP:51820/udp"
echo "Subnet:        10.0.0.0/24 (server=10.0.0.1)"
echo ""

# Start boringtun
BT_PAYMENT_SERVER=1 \
BT_HTTP_BIND="0.0.0.0:8089" \
BT_PUBLIC_IP="$PUBLIC_IP" \
BT_WS_BIND="0.0.0.0:8443" \
BT_WG_PORT=51820 \
WG_LOG_LEVEL=info \
WG_SUDO=1 \
    "$BT_BIN" wg0 --foreground --disable-drop-privileges &
BT_PID=$!
sleep 3
kill -0 $BT_PID 2>/dev/null || { echo "ERROR: boringtun exited"; exit 1; }

# Configure WireGuard via UAPI
echo "[Setup] Configuring WireGuard via UAPI..."
UAPI_RESULT=$(printf "set=1\nprivate_key=%s\nlisten_port=51820\n\n" "$SERVER_PRIVKEY_HEX" | \
    socat -t5 - UNIX-CONNECT:/var/run/wireguard/wg0.sock 2>&1)
echo "[Setup] UAPI response: $UAPI_RESULT"

# Interface + routing (Linux)
echo "[Setup] Configuring interface wg0..."
ip link set wg0 up 2>/dev/null || true
ip addr replace 10.0.0.1/24 dev wg0 2>/dev/null || true
ip route replace 10.0.0.0/24 dev wg0 2>/dev/null || true
echo "[Setup] Interface wg0: $(ip -4 addr show wg0 2>/dev/null | grep inet || echo 'no address')"

# IP forwarding + NAT
echo "[Setup] Enabling IP forwarding and NAT..."
sysctl -w net.ipv4.ip_forward=1 >/dev/null
INET_IF=$(ip route show default | awk '{print $5; exit}')
if [ -n "$INET_IF" ]; then
    iptables -t nat -C POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE 2>/dev/null || \
        iptables -t nat -A POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE
    echo "[Setup] NAT: 10.0.0.0/24 -> $INET_IF"
else
    echo "WARNING: Could not detect default interface for NAT"
fi

# Verify
echo ""
echo "[Verify] Testing health endpoint..."
HEALTH=$(curl -s --connect-timeout 3 "http://127.0.0.1:8089/health" 2>&1) || true
[ -n "$HEALTH" ] && echo "[Verify] Health: $HEALTH" || echo "[Verify] WARNING: Health endpoint not responding"

echo ""
echo "=== Server running ==="
echo "To register a client:"
echo "  curl -X POST http://$PUBLIC_IP:8089/v1/register -d '{\"public_key\":\"<base64_pubkey>\"}'"
echo ""
echo "Press Ctrl+C to stop"

cleanup() {
    echo "[Stop] Shutting down..."
    kill $BT_PID 2>/dev/null; wait $BT_PID 2>/dev/null
    iptables -t nat -D POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE 2>/dev/null || true
    sysctl -w net.ipv4.ip_forward=0 >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM
wait $BT_PID
