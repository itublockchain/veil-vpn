#!/bin/bash
# =============================================================================
# Veil VPN — Production Server (Linux)
# =============================================================================
#
# Usage:
#   sudo PUBLIC_IP=<your-vps-ip> ./scripts/server.sh
#
# Prereqs:
#   1. ./scripts/gen-keys.sh
#   2. cargo build --release -p boringtun-cli --features payment
#   3. apt install socat
#
# Ports:
#   8089/tcp  — HTTP API (registration)
#   8443/tcp  — WebSocket proxy (WireGuard over WS)
#   51820/udp — WireGuard (direct UDP, optional)

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
KEYS_DIR="$SCRIPT_DIR/keys"
IFACE="wg0"

[ -f "$KEYS_DIR/server.key" ] || { echo "ERROR: Run ./scripts/gen-keys.sh first"; exit 1; }
SERVER_PRIVKEY_HEX=$(cat "$KEYS_DIR/server.key" | base64 -d | xxd -p -c 64)

PUBLIC_IP="${PUBLIC_IP:?ERROR: Set PUBLIC_IP env var (e.g. PUBLIC_IP=1.2.3.4 ./scripts/server.sh)}"

BT_BIN="$ROOT_DIR/target/release/boringtun-cli"
[ -f "$BT_BIN" ] || BT_BIN="$ROOT_DIR/target/debug/boringtun-cli"
[ -f "$BT_BIN" ] || { echo "ERROR: Build first: cargo build --release -p boringtun-cli --features payment"; exit 1; }

# --- Cleanup stale state ---
pkill -f "boringtun.*${IFACE}" 2>/dev/null || true
sleep 1
ip link delete "$IFACE" 2>/dev/null || true
rm -f "/var/run/wireguard/${IFACE}.sock"

echo "=== Veil VPN Server ==="
echo "Public IP:     $PUBLIC_IP"
echo "HTTP API:      http://$PUBLIC_IP:8089"
echo "WebSocket:     ws://$PUBLIC_IP:8443"
echo "WireGuard:     $PUBLIC_IP:51820/udp"
echo "Subnet:        10.0.0.0/24 (server=10.0.0.1)"
echo ""

# --- Start boringtun ---
echo "[Start] Launching boringtun..."
BT_PAYMENT_SERVER=1 \
BT_HTTP_BIND="0.0.0.0:8089" \
BT_PUBLIC_IP="$PUBLIC_IP" \
BT_WS_BIND="0.0.0.0:8443" \
BT_WG_PORT=51820 \
WG_LOG_LEVEL=info \
WG_SUDO=1 \
    "$BT_BIN" "$IFACE" --foreground --disable-drop-privileges --disable-connected-udp &
BT_PID=$!
sleep 3
kill -0 $BT_PID 2>/dev/null || { echo "ERROR: boringtun exited"; exit 1; }

# --- Wait for UAPI socket ---
UAPI_SOCK="/var/run/wireguard/${IFACE}.sock"
for i in $(seq 1 10); do
    [ -S "$UAPI_SOCK" ] && break
    sleep 1
done
[ -S "$UAPI_SOCK" ] || { echo "ERROR: UAPI socket not found"; kill $BT_PID; exit 1; }

# --- Configure WireGuard via UAPI ---
echo "[Setup] Configuring WireGuard..."
UAPI_RESULT=$(printf "set=1\nprivate_key=%s\nlisten_port=51820\n\n" "$SERVER_PRIVKEY_HEX" | \
    socat -t5 - UNIX-CONNECT:"$UAPI_SOCK" 2>&1)
echo "[Setup] UAPI: $UAPI_RESULT"

# --- Interface ---
# Use /32 for the server address. Do NOT use /24 here — it causes a
# kernel-level routing loop: decrypted packets for non-existent VPN IPs
# get routed back into the TUN by the kernel (ip_forward=1), boringtun
# reads them again, can't find a peer, drops, but kernel already wrote
# another copy. With /32, only 10.0.0.1 is on-link.
echo "[Setup] Configuring interface..."
ip link set "$IFACE" up
ip addr replace 10.0.0.1/32 dev "$IFACE"

# --- IP Forwarding + NAT ---
echo "[Setup] Enabling IP forwarding and NAT..."
sysctl -w net.ipv4.ip_forward=1 >/dev/null

INET_IF=$(ip route show default | awk '{print $5; exit}')
if [ -n "$INET_IF" ]; then
    iptables -t nat -C POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE 2>/dev/null || \
        iptables -t nat -A POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE
    # Allow forwarding from/to wg0
    iptables -C FORWARD -i "$IFACE" -j ACCEPT 2>/dev/null || \
        iptables -A FORWARD -i "$IFACE" -j ACCEPT
    iptables -C FORWARD -o "$IFACE" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || \
        iptables -A FORWARD -o "$IFACE" -m state --state RELATED,ESTABLISHED -j ACCEPT
    echo "[Setup] NAT: 10.0.0.0/24 → $INET_IF"
else
    echo "WARNING: Could not detect default interface for NAT"
fi

# --- Verify ---
echo ""
echo "[Verify] Testing health endpoint..."
HEALTH=$(curl -s --connect-timeout 3 "http://127.0.0.1:8089/health" 2>&1) || true
[ -n "$HEALTH" ] && echo "[Verify] Health: $HEALTH" || echo "[Verify] WARNING: Health not responding"

echo ""
echo "=== Server running ==="
echo "Register a client:"
echo "  curl -X POST http://$PUBLIC_IP:8089/v1/register -d '{\"public_key\":\"<base64>\"}'"
echo ""
echo "Press Ctrl+C to stop"

# --- Cleanup ---
cleanup() {
    echo ""
    echo "[Stop] Shutting down..."
    kill $BT_PID 2>/dev/null
    wait $BT_PID 2>/dev/null
    ip link delete "$IFACE" 2>/dev/null || true
    if [ -n "$INET_IF" ]; then
        iptables -t nat -D POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE 2>/dev/null || true
        iptables -D FORWARD -i "$IFACE" -j ACCEPT 2>/dev/null || true
        iptables -D FORWARD -o "$IFACE" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || true
    fi
    echo "[Stop] Done."
}
trap cleanup EXIT INT TERM

wait $BT_PID
