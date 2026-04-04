#!/bin/bash
# Generate WireGuard keypairs for server and client testing.
# Writes to scripts/keys/ directory.

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
KEYS_DIR="$SCRIPT_DIR/keys"
mkdir -p "$KEYS_DIR"

gen_keypair() {
    local name=$1
    if command -v wg &>/dev/null; then
        wg genkey > "$KEYS_DIR/${name}.key"
        wg pubkey < "$KEYS_DIR/${name}.key" > "$KEYS_DIR/${name}.pub"
    else
        openssl genpkey -algorithm x25519 -outform DER 2>/dev/null | tail -c 32 | base64 > "$KEYS_DIR/${name}.key"
        echo "(public key requires 'wg' tool)" > "$KEYS_DIR/${name}.pub"
    fi
    echo "Generated $name keypair:"
    echo "  Private: $(cat "$KEYS_DIR/${name}.key")"
    echo "  Public:  $(cat "$KEYS_DIR/${name}.pub")"
}

gen_keypair "server"
gen_keypair "client"

echo ""
echo "Keys saved to $KEYS_DIR/"
echo ""
echo "Export for test scripts:"
echo "  export SERVER_PRIVKEY=$(cat "$KEYS_DIR/server.key")"
echo "  export SERVER_PUBKEY=$(cat "$KEYS_DIR/server.pub")"
echo "  export CLIENT_PRIVKEY=$(cat "$KEYS_DIR/client.key")"
echo "  export CLIENT_PUBKEY=$(cat "$KEYS_DIR/client.pub")"
