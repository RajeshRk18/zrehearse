#!/bin/sh
# A wallet project for zrehearse. It drives zcash-devtool, the CLI of the
# librustzcash wallet crates, through the light server.
# Usage: sh shielded-wallet.sh [sync]. With "sync" it does not send.
set -eu

fail() {
  echo "shielded-wallet: $*" >&2
  exit 1
}

command -v zcash-devtool >/dev/null || fail "zcash-devtool not on PATH. Install it with
  cargo install --locked --git https://github.com/zcash/zcash-devtool --rev 399fb4ee309afbdfd9e8bfda28ac4f555fdb94de --features regtest_support zcash-devtool"
[ -n "${ZREHEARSE_SHIELDED_MNEMONIC:-}" ] || fail "ZREHEARSE_SHIELDED_MNEMONIC is not set. Use the default shielded address."
[ -n "${ZREHEARSE_LIGHTWALLETD_URL:-}" ] || fail "ZREHEARSE_LIGHTWALLETD_URL is not set. The plan needs a [light_server]."

key() {
  case "$1" in
    NU5 | NU6 | NU6.1 | NU6.2 | NU6.3 | NU7) echo "$1" | tr 'A-Z.' 'a-z_' ;;
    *) fail "unknown upgrade $1" ;;
  esac
}
target=$(key "$ZREHEARSE_UPGRADE")
previous=$(key "$ZREHEARSE_PREVIOUS_UPGRADE")

rpc() {
  curl -sf -X POST -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" "$ZREHEARSE_RPC_URL" ||
    fail "RPC $1 failed"
}

W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
server=${ZREHEARSE_LIGHTWALLETD_URL#http://}

# Zebra regtest activates Overwinter to Canopy at 1. zrehearse activates
# NU5 to previous at 2 and the target at the activation height.
{
  for k in overwinter sapling blossom heartwood canopy; do echo "$k = 1"; done
  for k in nu5 nu6 nu6_1 nu6_2 nu6_3 nu7; do
    echo "$k = 2"
    [ "$k" = "$previous" ] && break
  done
  echo "$target = $ZREHEARSE_ACTIVATION_HEIGHT"
} >"$W/heights.toml"

wallet() {
  zcash-devtool wallet -w "$W" "$@"
}

# lightwalletd has no tree state at height 1, so the birthday is 2.
echo "$ZREHEARSE_SHIELDED_MNEMONIC" | wallet restore-mnemonic --name a0 -i "$W/id.txt" -n regtest \
  --activation-heights "$W/heights.toml" -s "$server" --birthday 2 >/dev/null ||
  fail "restore-mnemonic failed"
wallet sync -s "$server" >/dev/null || fail "sync failed"

balance=$(wallet balance --json --min-confirmations 1) || fail "balance failed"
field() {
  echo "$balance" | sed -n "s/.*\"$1\":\([0-9]*\).*/\1/p"
}
sapling=$(field sapling_spendable)
orchard=$(field orchard_spendable)
ironwood=$(field ironwood_spendable)
echo "balance at tip $(field chain_tip_height): sapling $sapling, orchard $orchard, ironwood $ironwood"
[ $((sapling + orchard + ironwood)) -gt 0 ] || fail "no shielded balance"
# NU6.3 moves the shielded rewards from Orchard to Ironwood.
if [ "$ZREHEARSE_UPGRADE" = NU6.3 ]; then
  [ "$orchard" -gt 0 ] || fail "no Orchard balance from before activation"
  [ "$ironwood" -gt 0 ] || fail "no Ironwood balance from after activation"
fi

[ "${1:-}" = sync ] && exit 0

# Send 1 ZEC more than either pool holds, so the wallet must spend notes from
# both sides of the boundary.
value=$((100000000 + (ironwood > orchard ? ironwood : orchard)))
wallet send -i "$W/id.txt" --address "$ZREHEARSE_SHIELDED_ADDRESS" --value "$value" \
  -s "$server" --min-confirmations 1 >"$W/send.log" || fail "send failed. $(cat "$W/send.log")"
txid=$(tail -n 1 "$W/send.log")
echo "sent $value zatoshi in $txid"

tries=0
until rpc getrawmempool '[]' | grep -q "$txid"; do
  tries=$((tries + 1))
  [ "$tries" -lt 30 ] || fail "$txid did not reach the mempool"
  sleep 1
done
rpc generate '[1]' >/dev/null
rpc getrawmempool '[]' | grep -q "$txid" && fail "$txid is still in the mempool after a block"
tx=$(rpc getrawtransaction "[\"$txid\",1]")
echo "$tx" | grep -q '"confirmations":1' || fail "$txid is not in a block"
if [ "$ZREHEARSE_UPGRADE" = NU6.3 ]; then
  for pool in orchard ironwood; do
    echo "$tx" | grep -q "\"$pool\":{\"actions\":\[{" || fail "$txid has no $pool actions"
  done
  echo "$txid spends Orchard and Ironwood notes"
fi
echo "mined $txid"
