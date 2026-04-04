#!/bin/bash
RPC="https://rpc.testnet.arc.network"
GW="0x0077777d7EBA4688BDeF3E311b846F25870A19B9"
CLIENT="171763bc5106ddfc6d62aa62715777f4b5d930d9"
SERVER="03491b70e66500fc91e67391f51a331403b56add"

query() {
  local addr=$1
  local hex=$(curl -s -X POST "$RPC" -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_call\",\"params\":[{\"to\":\"$GW\",\"data\":\"0x3ccb64ae0000000000000000000000003600000000000000000000000000000000000000000000000000000000000000${addr}\"},\"latest\"],\"id\":1}" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['result'])")
  python3 -c "v=int('$hex',16); print(f'{v/1e6:.6f} USDC  ({v} raw)')"
}

echo "Client (0x$CLIENT):"
echo "  Gateway: $(query $CLIENT)"
echo ""
echo "Server (0x$SERVER):"
echo "  Gateway: $(query $SERVER)"
