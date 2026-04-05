#!/bin/bash
# =============================================================================
# Veil VPN — ROFL TEE Container Entrypoint
# =============================================================================
# Runs boringtun WireGuard server inside a ROFL TEE container.
# Keys are generated inside the enclave and never leave it.
set -e

IFACE="tun0"
UAPI_SOCK="/var/run/wireguard/${IFACE}.sock"
KEYS_DIR="/app/data/keys"

# --- Key Generation (inside TEE, never exported) ---
generate_keys() {
    mkdir -p "$KEYS_DIR"
    if [ -f "$KEYS_DIR/server.key" ]; then
        echo "[Keys] Using existing keypair from sealed storage"
    else
        echo "[Keys] Generating WireGuard keypair inside TEE..."
        openssl genpkey -algorithm x25519 -outform DER 2>/dev/null | tail -c 32 | base64 > "$KEYS_DIR/server.key"
        echo "[Keys] Private key generated (sealed to enclave)"
    fi

    SERVER_PRIVKEY=$(cat "$KEYS_DIR/server.key")
    SERVER_PRIVKEY_HEX=$(echo -n "$SERVER_PRIVKEY" | base64 -d | xxd -p -c 64)

    # Derive public key: create full DER private key, extract public
    # x25519 public key derivation via openssl
    SERVER_PUBKEY=$(echo -n "$SERVER_PRIVKEY" | base64 -d | \
        openssl pkey -inform DER -outform DER -pubout 2>/dev/null | tail -c 32 | base64)

    if [ -z "$SERVER_PUBKEY" ]; then
        # Fallback: boringtun will log its public key, we extract later
        SERVER_PUBKEY="(derived at runtime)"
    fi

    echo "[Keys] Server public key: $SERVER_PUBKEY"
    echo "$SERVER_PUBKEY" > "$KEYS_DIR/server.pub"
}

# --- Cleanup ---
cleanup() {
    echo ""
    echo "[Stop] Shutting down..."
    kill $BT_PID 2>/dev/null || true
    wait $BT_PID 2>/dev/null || true
    ip link delete "$IFACE" 2>/dev/null || true
    INET_IF=$(ip route show default | awk '{print $5; exit}')
    if [ -n "$INET_IF" ]; then
        iptables -t nat -D POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE 2>/dev/null || true
        iptables -D FORWARD -i "$IFACE" -j ACCEPT 2>/dev/null || true
        iptables -D FORWARD -o "$IFACE" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || true
    fi
    echo "[Stop] Done."
}
trap cleanup EXIT INT TERM

echo "=== TEE VPN Node Starting ==="

# 1. Generate keys inside enclave
generate_keys

# 2. Setup TUN interface
echo "[Setup] Creating TUN interface..."
ip tuntap add dev "$IFACE" mode tun 2>/dev/null || true
ip link set "$IFACE" up

# 3. Start boringtun
echo "[Start] Launching boringtun..."
BT_PAYMENT_SERVER=1 \
BT_HTTP_BIND="${BT_HTTP_BIND:-0.0.0.0:8080}" \
BT_PUBLIC_IP="${BT_PUBLIC_IP:-${PUBLIC_IP:-127.0.0.1}}" \
BT_WS_BIND="${BT_WS_BIND:-0.0.0.0:8443}" \
BT_WG_PORT="${BT_WG_PORT:-51820}" \
WG_LOG_LEVEL="${WG_LOG_LEVEL:-info}" \
WG_SUDO=1 \
boringtun "$IFACE" --foreground --disable-drop-privileges --disable-connected-udp &

BT_PID=$!
sleep 2
kill -0 $BT_PID 2>/dev/null || { echo "ERROR: boringtun exited immediately"; exit 1; }

# 4. Wait for UAPI socket
echo "[Setup] Waiting for UAPI socket..."
for i in $(seq 1 10); do
    [ -S "$UAPI_SOCK" ] && break
    sleep 1
done
[ -S "$UAPI_SOCK" ] || { echo "ERROR: UAPI socket not found after 10s"; kill $BT_PID; exit 1; }
echo "[Setup] UAPI socket ready: $UAPI_SOCK"

# 5. Configure WireGuard via UAPI
echo "[Setup] Configuring WireGuard..."
UAPI_RESULT=$(printf "set=1\nprivate_key=%s\nlisten_port=51820\n\n" "$SERVER_PRIVKEY_HEX" | \
    socat -t5 - UNIX-CONNECT:"$UAPI_SOCK" 2>&1)
echo "[Setup] UAPI: $UAPI_RESULT"

# 6. Interface addressing — /32 to prevent routing loops
echo "[Setup] Configuring interface..."
ip addr replace 10.0.0.1/32 dev "$IFACE"

# 7. IP forwarding + NAT
echo "[Setup] Enabling IP forwarding and NAT..."
sysctl -w net.ipv4.ip_forward=1 >/dev/null

INET_IF=$(ip route show default | awk '{print $5; exit}')
if [ -n "$INET_IF" ]; then
    iptables -t nat -C POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE 2>/dev/null || \
        iptables -t nat -A POSTROUTING -s 10.0.0.0/24 -o "$INET_IF" -j MASQUERADE
    iptables -C FORWARD -i "$IFACE" -j ACCEPT 2>/dev/null || \
        iptables -A FORWARD -i "$IFACE" -j ACCEPT
    iptables -C FORWARD -o "$IFACE" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || \
        iptables -A FORWARD -o "$IFACE" -m state --state RELATED,ESTABLISHED -j ACCEPT
    echo "[Setup] NAT: 10.0.0.0/24 → $INET_IF"
else
    echo "WARNING: Could not detect default interface for NAT"
fi

# 8. Health check
echo "[Verify] Testing health endpoint..."
for i in $(seq 1 5); do
    HEALTH=$(curl -s --connect-timeout 2 "http://127.0.0.1:${HTTP_PORT:-8080}/health" 2>&1) && break
    sleep 1
done
[ -n "$HEALTH" ] && echo "[Verify] Health: $HEALTH" || echo "[Verify] WARNING: Health not responding"

# 9. TEE registration (ENS + Sapphire attestation)
echo "[Boot] Running TEE registration..."
export WG_PUBLIC_KEY="$SERVER_PUBKEY"
cd /app/tee-service
npx tsx src/register.ts || echo "[Boot] Registration failed (non-fatal, node runs unattested)"

echo ""
echo "=== TEE VPN Node Running ==="
echo "HTTP API:  ${BT_HTTP_BIND:-0.0.0.0:8080}"
echo "WS Proxy:  ${BT_WS_BIND:-0.0.0.0:8443}"
echo "Public IP: ${BT_PUBLIC_IP:-${PUBLIC_IP:-127.0.0.1}}"
echo "Server PK: $SERVER_PUBKEY"

# Wait for boringtun (main process)
wait $BT_PID
