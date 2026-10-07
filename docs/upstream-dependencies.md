# Upstream dependencies

zrehearse drives Zebra, lightwalletd, Zaino and grpcurl through their images, flags, env vars and RPCs. This page lists each such interface, how likely it is to change, and where the code that uses it is. Verified on 2026-10-06 against `zfnd/zebra:6.2.3`, `zfnd/zebra:7.0.0-rc.0`, `electriccoinco/lightwalletd:v0.5.4` and `zingodevops/zaino:0.10.1-no-tls`.

## Parts that do not depend on a version

- The light server and grpcurl share zebrad's network namespace with `--network container:<zebrad>`. This is a stable Docker feature. zrehearse creates no Docker network, so a host with no free address pools does not stop it.
- Light server readiness waits for `GetBlock` at the target height. A served block is part of the protocol, so this does not depend on how a server reports its height.
- Removal of color codes and the merge of grpcurl errors into one line change only report text. A new log format does not break a check.
- zrehearse has no list of network upgrades. The plan names `previous` and `name`, and Zebra checks them. A new upgrade needs no change to zrehearse.
- grpcurl is pinned to `fullstorydev/grpcurl:v1.9.3`, and the protos in `proto/` are pinned to lightwallet-protocol v0.5.0 (commit ac7cee05).

## Interfaces that can change

| Upstream | Interface | Risk | Code |
|---|---|---|---|
| Zaino | Config file at `/app/config/zainod.toml`, env vars `ZAINO_VALIDATOR_SETTINGS__VALIDATOR_JSONRPC_LISTEN_ADDRESS` and `ZAINO_GRPC_SETTINGS__LISTEN_ADDRESS`, the `-no-tls` image tags, gRPC port 8137 | High. Zaino is at 0.x and changes quickly. Its docs say env vars alone work, but 0.10.1 needs the file. | `src/lightserver.rs` |
| lightwalletd | Flags `--no-tls-very-insecure`, `--grpc-bind-addr`, `--rpchost`, `--rpcport`, `--rpcuser`, `--rpcpassword`, `--data-dir`, `--log-file`, gRPC port 9067 | Low to medium. These flags are old. | `src/lightserver.rs` |
| Zebra | Config path `/home/zebra/.config/zebrad.toml`, env vars `ZEBRA_RPC__LISTEN_ADDR`, `ZEBRA_RPC__ENABLE_COOKIE_AUTH` and `ZEBRA_MINING__MINER_ADDRESS`, the regtest-only `generate` RPC | Medium. A major release can rename them. | `src/node.rs` |
| Zebra | Config key `network.testnet_parameters.activation_heights` | Medium. | `src/plan.rs` |
| Zebra | RPC shapes of `getblockchaininfo` (upgrades by branch ID, `consensus.chaintip` and `nextblock`), `getblock`, `getaddressutxos`, `z_gettreestate`, `z_getsubtreesbyindex` | Low. Wallets and light servers depend on them too. | `src/rehearse.rs` |
| Zebra | Regtest behavior. Upgrades below `previous` get the next configured height, and block rewards can be spent to transparent outputs. | Low to medium. Verified on 6.2.3 and 7.0.0-rc.0 only. | `src/plan.rs`, `src/rehearse.rs` |
| Zakura | Reads the config at Zebra's path `/home/zebra/.config/zebrad.toml` (it warns that the name is deprecated) and maps the legacy `ZEBRA_*` env vars to `ZAKURA_*` | Medium. A later release can drop the old path or the env var mapping. Then zrehearse needs a config path setting. | `src/node.rs` |
| Zebra | Coinbase maturity of 100 blocks | Low. It is a consensus rule. | `src/plan.rs` |
| lightwallet-protocol | Method names and the fields zrehearse reads (`blockHeight`, `consensusBranchId`, `hash`, `saplingTree`, `orchardTree`) | Low. New fields do not break zrehearse. | `src/lightserver.rs`, `src/rehearse.rs` |
| Zcash consensus | A v5 transaction stays valid after the upgrade | Low. Only `examples/spend.rs` depends on it. A future upgrade that rejects v5 needs a v6 builder there. | `examples/spend.rs` |

## How a break shows

Each break above gives a failed check, or a setup error with exit code 2, together with the container's log in the output directory. A break does not make a rehearsal pass when it should fail.

CI tests only the pinned image tags. Thus it does not find a change in a new upstream release. A new release breaks zrehearse only when a plan points at it.

## Finding drift early

This is not built yet. A CI job that runs once a week, separate from the normal CI so that it cannot block a merge, could run the examples against the newest Zebra, lightwalletd and Zaino releases. The first upstream change that breaks zrehearse would then show within a week. The fix is usually one flag or one env var name in the files in the table above. The tags that each project uses for its newest release are not verified yet.
