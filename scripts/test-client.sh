#!/bin/bash
# Start boringtun client locally, connecting to server via WebSocket proxy.
#
# Modes:
#   Full tunnel (all traffic via remote host):
#     sudo FULL_TUNNEL=1 ./scripts/test-client.sh
#
#   Local only (VPN subnet only, default):
#     sudo ./scripts/test-client.sh

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
KEYS_DIR="$SCRIPT_DIR/keys"

SERVER_HOST="${SERVER_HOST:-127.0.0.1}"
SERVER_HTTP_PORT="${SERVER_HTTP_PORT:-8089}"
SERVER_WS_PORT="${SERVER_WS_PORT:-8443}"
LOCAL_UDP_PORT=51821
FULL_TUNNEL="${FULL_TUNNEL:-0}"

# --- Keys ---
[ -f "$KEYS_DIR/client.key" ] || { echo "ERROR: Run ./scripts/gen-keys.sh first"; exit 1; }
CLIENT_PRIVKEY_HEX=$(cat "$KEYS_DIR/client.key" | base64 -d | xxd -p -c 64)
CLIENT_PUBKEY=$(cat "$KEYS_DIR/client.pub")
SERVER_PUBKEY_HEX=$(cat "$KEYS_DIR/server.pub" | base64 -d | xxd -p -c 64)

# --- Register with server ---
echo "[Register] Checking server..."
curl -sf --connect-timeout 3 "http://${SERVER_HOST}:${SERVER_HTTP_PORT}/health" >/dev/null || \
    { echo "ERROR: Server not reachable"; exit 1; }

REGISTER_RESPONSE=$(curl -sf -X POST "http://${SERVER_HOST}:${SERVER_HTTP_PORT}/v1/register" \
    -H "Content-Type: application/json" \
    -d "{\"public_key\":\"${CLIENT_PUBKEY}\"}") || { echo "ERROR: Registration failed"; exit 1; }

ASSIGNED_IP=$(echo "$REGISTER_RESPONSE" | grep -o '"assigned_ip":"[^"]*"' | cut -d'"' -f4 | cut -d'/' -f1)
echo "[Register] Assigned IP: $ASSIGNED_IP"

# --- Boringtun client with built-in WS bridge ---
BT_BIN="$ROOT_DIR/target/debug/boringtun-cli"
[ -f "$ROOT_DIR/target/release/boringtun-cli" ] && BT_BIN="$ROOT_DIR/target/release/boringtun-cli"

BEFORE_SOCKS=$(ls /var/run/wireguard/utun*.sock 2>/dev/null || true)

BT_WS_CONNECT="ws://${SERVER_HOST}:${SERVER_WS_PORT}" \
BT_WS_LOCAL_PORT="$LOCAL_UDP_PORT" \
WG_LOG_LEVEL=info WG_SUDO=1 "$BT_BIN" utun --foreground --disable-drop-privileges &
BT_PID=$!
sleep 3
kill -0 $BT_PID 2>/dev/null || { echo "ERROR: boringtun exited"; exit 1; }

# Find new socket
UTUN_NAME=""
for sock in /var/run/wireguard/utun*.sock; do
    [ -e "$sock" ] || continue
    echo "$BEFORE_SOCKS" | grep -q "$sock" && continue
    UTUN_NAME=$(basename "$sock" .sock)
    break
done
[ -n "$UTUN_NAME" ] || { echo "ERROR: No UAPI socket found"; kill $BT_PID; exit 1; }

echo "[Setup] Interface: $UTUN_NAME"

# --- Configure WireGuard ---
if [ "$FULL_TUNNEL" = "1" ]; then
    ALLOWED_IPS="allowed_ip=0.0.0.0/0"
else
    ALLOWED_IPS="allowed_ip=10.0.0.0/24"
fi

printf "set=1\nprivate_key=%s\npublic_key=%s\nendpoint=127.0.0.1:%s\n%b\npersistent_keepalive_interval=25\n\n" \
    "$CLIENT_PRIVKEY_HEX" "$SERVER_PUBKEY_HEX" "$LOCAL_UDP_PORT" "$ALLOWED_IPS" | \
    socat -t5 - UNIX-CONNECT:"/var/run/wireguard/${UTUN_NAME}.sock" >/dev/null 2>&1

# --- Routing ---
ifconfig "$UTUN_NAME" "$ASSIGNED_IP" 10.0.0.1 up

if [ "$FULL_TUNNEL" = "1" ]; then
    echo "[Route] Full tunnel — all traffic through remote host"
    ORIG_GW=$(route -n get default 2>/dev/null | awk '/gateway:/{print $2}')
    if [ -n "$ORIG_GW" ]; then
        route add -host "$SERVER_HOST" "$ORIG_GW" 2>/dev/null || true
    fi
    route add -net 0.0.0.0/1 -interface "$UTUN_NAME" 2>/dev/null || true
    route add -net 128.0.0.0/1 -interface "$UTUN_NAME" 2>/dev/null || true
else
    echo "[Route] Local only — VPN subnet (10.0.0.0/24)"
    route add -net 10.0.0.0/24 -interface "$UTUN_NAME" 2>/dev/null || true
fi

echo ""
echo "=== Client running ==="
echo "Client IP:   $ASSIGNED_IP"
echo "Mode:        $([ "$FULL_TUNNEL" = "1" ] && echo "full tunnel (remote)" || echo "local only")"
echo ""
echo "Test:"
echo "  ping 10.0.0.1"
if [ "$FULL_TUNNEL" = "1" ]; then
    echo "  ping 8.8.8.8"
    echo "  curl ifconfig.me"
fi
echo ""

cleanup() {
    echo "[Stop] Cleaning up..."
    if [ "$FULL_TUNNEL" = "1" ]; then
        route delete -host "$SERVER_HOST" 2>/dev/null || true
        route delete -net 0.0.0.0/1 2>/dev/null || true
        route delete -net 128.0.0.0/1 2>/dev/null || true
    else
        route delete -net 10.0.0.0/24 2>/dev/null || true
    fi
    kill $BT_PID 2>/dev/null
    wait $BT_PID 2>/dev/null
}
trap cleanup EXIT INT TERM

wait $BT_PID
