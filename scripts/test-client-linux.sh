#!/bin/bash
# Start boringtun client on Linux, connecting to server via WebSocket proxy.
#
# Modes:
#   Full tunnel (all traffic via remote host):
#     sudo FULL_TUNNEL=1 ./scripts/test-client-linux.sh
#
#   Local only (VPN subnet only, default):
#     sudo ./scripts/test-client-linux.sh

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
KEYS_DIR="$SCRIPT_DIR/keys"

SERVER_HOST="${SERVER_HOST:-37.27.29.160}"
SERVER_HTTP_PORT="${SERVER_HTTP_PORT:-8089}"
SERVER_WS_PORT="${SERVER_WS_PORT:-8443}"
LOCAL_UDP_PORT=51821
FULL_TUNNEL="${FULL_TUNNEL:-0}"
IFACE="wg0"

# --- Keys ---
[ -f "$KEYS_DIR/client.key" ] || { echo "ERROR: Run ./scripts/gen-keys.sh first"; exit 1; }
CLIENT_PRIVKEY_HEX=$(cat "$KEYS_DIR/client.key" | base64 -d | xxd -p -c 64)
CLIENT_PUBKEY=$(cat "$KEYS_DIR/client.pub")
SERVER_PUBKEY_HEX=$(cat "$KEYS_DIR/server.pub" | base64 -d | xxd -p -c 64)

# --- Binary ---
BT_BIN="$ROOT_DIR/target/release/boringtun-cli"
[ -f "$BT_BIN" ] || BT_BIN="$ROOT_DIR/target/debug/boringtun-cli"
[ -f "$BT_BIN" ] || { echo "ERROR: Build first: cargo build -p boringtun-cli --features payment"; exit 1; }

# --- Cleanup stale state ---
pkill -f "boringtun.*${IFACE}" 2>/dev/null || true
sleep 1
ip link delete "$IFACE" 2>/dev/null || true
rm -f "/var/run/wireguard/${IFACE}.sock"

# --- Register with server ---
echo "[Register] Checking server at ${SERVER_HOST}:${SERVER_HTTP_PORT}..."
curl -sf --connect-timeout 5 "http://${SERVER_HOST}:${SERVER_HTTP_PORT}/health" >/dev/null || \
    { echo "ERROR: Server not reachable at http://${SERVER_HOST}:${SERVER_HTTP_PORT}/health"; exit 1; }

REGISTER_RESPONSE=$(curl -sf -X POST "http://${SERVER_HOST}:${SERVER_HTTP_PORT}/v1/register" \
    -H "Content-Type: application/json" \
    -d "{\"public_key\":\"${CLIENT_PUBKEY}\"}") || { echo "ERROR: Registration failed"; exit 1; }

ASSIGNED_IP=$(echo "$REGISTER_RESPONSE" | grep -o '"assigned_ip":"[^"]*"' | cut -d'"' -f4 | cut -d'/' -f1)
echo "[Register] Assigned IP: $ASSIGNED_IP"

# --- Start boringtun with WS bridge ---
echo "[Start] Launching boringtun (WS bridge → ${SERVER_HOST}:${SERVER_WS_PORT})..."
WG_LOG_LEVEL=debug \
WG_SUDO=1 \
BT_WS_CONNECT="ws://${SERVER_HOST}:${SERVER_WS_PORT}" \
BT_WS_LOCAL_PORT="$LOCAL_UDP_PORT" \
    "$BT_BIN" "$IFACE" --foreground --disable-drop-privileges \
    --ws-connect "ws://${SERVER_HOST}:${SERVER_WS_PORT}" \
    --ws-local-port "$LOCAL_UDP_PORT" &
BT_PID=$!
sleep 3

if ! kill -0 $BT_PID 2>/dev/null; then
    echo "ERROR: boringtun exited. Check /tmp/boringtun.out for details."
    exit 1
fi

echo "[Setup] Interface: $IFACE (PID: $BT_PID)"

# --- Wait for UAPI socket ---
UAPI_SOCK="/var/run/wireguard/${IFACE}.sock"
for i in $(seq 1 10); do
    [ -S "$UAPI_SOCK" ] && break
    sleep 1
done
[ -S "$UAPI_SOCK" ] || { echo "ERROR: UAPI socket not found at $UAPI_SOCK"; kill $BT_PID; exit 1; }

# --- Configure WireGuard ---
if [ "$FULL_TUNNEL" = "1" ]; then
    ALLOWED_IPS="allowed_ip=0.0.0.0/0"
else
    ALLOWED_IPS="allowed_ip=10.0.0.0/24"
fi

echo "[Setup] Configuring WireGuard peer..."
printf "set=1\nprivate_key=%s\npublic_key=%s\nendpoint=127.0.0.1:%s\n%b\npersistent_keepalive_interval=25\n\n" \
    "$CLIENT_PRIVKEY_HEX" "$SERVER_PUBKEY_HEX" "$LOCAL_UDP_PORT" "$ALLOWED_IPS" | \
    socat -t5 - UNIX-CONNECT:"$UAPI_SOCK" >/dev/null 2>&1

# --- Interface + Routing ---
echo "[Setup] Configuring interface..."
ip link set "$IFACE" up
ip addr replace "${ASSIGNED_IP}/24" dev "$IFACE"

ORIG_GW=""
ORIG_IF=""

if [ "$FULL_TUNNEL" = "1" ]; then
    echo "[Route] Full tunnel — all traffic through remote host"
    ORIG_GW=$(ip route show default | awk '{print $3; exit}')
    ORIG_IF=$(ip route show default | awk '{print $5; exit}')

    # Keep server reachable via original gateway
    if [ -n "$ORIG_GW" ]; then
        ip route add "$SERVER_HOST" via "$ORIG_GW" dev "$ORIG_IF" 2>/dev/null || true
    fi

    # Override default: 0/1 + 128/1 trick
    ip route add 0.0.0.0/1 dev "$IFACE" 2>/dev/null || true
    ip route add 128.0.0.0/1 dev "$IFACE" 2>/dev/null || true

    # Set DNS
    if command -v resolvconf >/dev/null 2>&1; then
        printf "nameserver 1.1.1.1\nnameserver 8.8.8.8\n" | resolvconf -a "$IFACE" 2>/dev/null || true
    fi
else
    echo "[Route] Local only — VPN subnet (10.0.0.0/24)"
    ip route replace 10.0.0.0/24 dev "$IFACE" 2>/dev/null || true
fi

echo ""
echo "=== Client running ==="
echo "Client IP:   $ASSIGNED_IP"
echo "Interface:   $IFACE"
echo "WS Bridge:   127.0.0.1:$LOCAL_UDP_PORT → ws://${SERVER_HOST}:${SERVER_WS_PORT}"
echo "Mode:        $([ "$FULL_TUNNEL" = "1" ] && echo "full tunnel (remote)" || echo "local only")"
echo ""
echo "Test:"
echo "  ping 10.0.0.1"
if [ "$FULL_TUNNEL" = "1" ]; then
    echo "  ping 8.8.8.8"
    echo "  curl ifconfig.me"
fi
echo ""
echo "Press Ctrl+C to stop"

# --- Cleanup ---
cleanup() {
    echo ""
    echo "[Stop] Cleaning up..."
    if [ "$FULL_TUNNEL" = "1" ]; then
        ip route delete "$SERVER_HOST" 2>/dev/null || true
        ip route delete 0.0.0.0/1 dev "$IFACE" 2>/dev/null || true
        ip route delete 128.0.0.0/1 dev "$IFACE" 2>/dev/null || true
        if command -v resolvconf >/dev/null 2>&1; then
            resolvconf -d "$IFACE" 2>/dev/null || true
        fi
    else
        ip route delete 10.0.0.0/24 dev "$IFACE" 2>/dev/null || true
    fi
    ip link delete "$IFACE" 2>/dev/null || true
    kill $BT_PID 2>/dev/null
    wait $BT_PID 2>/dev/null
    echo "[Stop] Done."
}
trap cleanup EXIT INT TERM

# --- Keep alive: monitor boringtun and restart if it dies ---
while true; do
    if ! kill -0 $BT_PID 2>/dev/null; then
        echo "[Monitor] boringtun died, restarting..."
        rm -f "$UAPI_SOCK"

        WG_LOG_LEVEL=debug \
        WG_SUDO=1 \
        BT_WS_CONNECT="ws://${SERVER_HOST}:${SERVER_WS_PORT}" \
        BT_WS_LOCAL_PORT="$LOCAL_UDP_PORT" \
            "$BT_BIN" "$IFACE" --foreground --disable-drop-privileges \
            --ws-connect "ws://${SERVER_HOST}:${SERVER_WS_PORT}" \
            --ws-local-port "$LOCAL_UDP_PORT" &
        BT_PID=$!
        sleep 3

        if ! kill -0 $BT_PID 2>/dev/null; then
            echo "[Monitor] boringtun failed to restart, exiting."
            exit 1
        fi

        # Wait for socket and reconfigure
        for i in $(seq 1 10); do
            [ -S "$UAPI_SOCK" ] && break
            sleep 1
        done

        if [ -S "$UAPI_SOCK" ]; then
            printf "set=1\nprivate_key=%s\npublic_key=%s\nendpoint=127.0.0.1:%s\n%b\npersistent_keepalive_interval=25\n\n" \
                "$CLIENT_PRIVKEY_HEX" "$SERVER_PUBKEY_HEX" "$LOCAL_UDP_PORT" "$ALLOWED_IPS" | \
                socat -t5 - UNIX-CONNECT:"$UAPI_SOCK" >/dev/null 2>&1

            ip link set "$IFACE" up 2>/dev/null || true
            ip addr replace "${ASSIGNED_IP}/24" dev "$IFACE" 2>/dev/null || true
            echo "[Monitor] boringtun restarted and reconfigured."
        else
            echo "[Monitor] UAPI socket not found after restart, exiting."
            exit 1
        fi
    fi
    sleep 5
done
