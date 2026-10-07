<p align="center"><img src="docs/zrehearse.svg" alt="zrehearse" width="720"></p>

# zrehearse

Network upgrade rehearsals for downstream Zcash projects.

A Zcash network upgrade changes the consensus rules at a fixed block height ([ZIP 200](https://zips.z.cash/zip-0200)). Each upgrade has its own consensus branch ID, and transaction signatures commit to it. Wallets, SDKs and light servers that build transactions or serve blocks must follow the new rules from the activation height onward. If they do not, they break when the upgrade activates.

**zrehearse moves that moment to your CI, or a local integration test via CLI.** It starts a throwaway regtest Zebra node with one upgrade set to activate at a height you pick. It mines across that height and checks that the upgrade really took effect. Then it runs your own test command against the node and reports pass or fail.

## Quick start

You need Docker and a Rust toolchain that supports edition 2024.

```sh
docker pull zfnd/zebra:6.2.3
cargo run -- run examples/nu6_3.toml
```

Output from a real run:

```
rehearsing NU6.3 at height 110 on zfnd/zebra:6.2.3
PASS  previous upgrade in force before activation  (tip 109, chaintip 5437f330, expected NU6.2 branch 5437f330)
PASS  pending one block before activation  (tip 109, status "pending", nextblock 37a5165b, expected branch 37a5165b)
PASS  active at the activation height  (tip 110, status "active", chaintip 37a5165b)
PASS  activation block is readable  (hash cb3d9022865db421b7c36582f461571b9f4e78209db7e0b281df4b71208261aa)
PASS  chain grows after activation  (tip 120, expected 120)
PASS  funded key has a spendable block reward  (21 spendable outputs for tmR9hqpF8N5vRERT3esi6PCfSGe1b81huxW at tip 120)
PASS  shielded rewards cross the boundary  (109 orchard 1, 110 ironwood 1)
PASS  project rpc-smoke  (exit 0, 0.2s, log out/nu6_3/project-rpc-smoke.log)
PASS  project spend  (exit 0, 1s, log out/nu6_3/project-spend.log)
REHEARSAL PASSED  report: out/nu6_3/report.json
```

## Usage

Install the CLI with cargo, or download a binary for Linux x64, Linux arm64 or macOS arm64 from the [releases](https://github.com/RajeshRk18/zrehearse/releases). You need Docker on the machine that runs it.

```sh
cargo install --locked --git https://github.com/RajeshRk18/zrehearse
```

```
zrehearse run <plan.toml> [--image <ref>] [--out <dir>] [--keep]
zrehearse --version
zrehearse --help
```

| Argument | Meaning |
|---|---|
| `<plan.toml>` | The plan file, described below |
| `--image <ref>` | Zebra image to run instead of `node.image` in the plan |
| `--out <dir>` | Where to write `report.json` and the logs. The default is `out/<plan file name>/`. |
| `--keep` | Leave the zebrad container running after the run. zrehearse prints its name and RPC URL. |

## The plan file

```toml
name = "nu6.3 on zebra 6.2.3"

[node]
image = "zfnd/zebra:6.2.3"

[upgrade]
name = "NU6.3"
previous = "NU6.2"

[[project]]
name = "my-wallet"
run = "cargo test --test upgrade -- --nocapture"
timeout_secs = 900
```

See [`examples/`](examples/) for reference plan files.

| Field | Default | Meaning |
|---|---|---|
| `name` | required | Free text, copied into the report |
| `node.image` | required | Zebra or Zakura image. It must know both upgrade names. |
| `node.ready_timeout_secs` | `120` | Seconds to wait for the node's RPC |
| `node.config` | none | TOML merged into the node config |
| `node.env` | none | Extra env vars for the node |
| `upgrade.name` | required | Upgrade under test, for example `NU6.3`. NU5 or later. |
| `upgrade.previous` | required | Upgrade active before it, for example `NU6.2` |
| `upgrade.height` | `110` | Activation height, 3 or more. `height + blocks_after` must be above 100. |
| `upgrade.blocks_after` | `10` | Blocks to mine after activation |
| `project.name` | required | Letters, digits, `-` and `_`. It names the log file. |
| `project.run` | required | Command for `sh -c`, run from the plan file's directory |
| `project.timeout_secs` | `600` | Seconds before the project fails. Processes that `sh` started keep running. |
| `funding.shielded_address` | public test address | Regtest Unified Address (`uregtest1...`) for the shielded rewards |
| `light_server.kind` | none | `lightwalletd` or `zaino`. Leave out `[light_server]` for no light server. |
| `light_server.image` | required with `kind` | For example `electriccoinco/lightwalletd:v0.5.4`. Zaino needs a `-no-tls` tag. |
| `light_server.ready_timeout_secs` | `120` | Seconds to wait for the light server to serve each tip |
| `light_server.config` | none | TOML merged into `zainod.toml`. Zaino only. |
| `light_server.env` | none | Extra env vars for the light server |
| `light_server.args` | none | Extra arguments, for example lightwalletd flags |

A plan cannot set the network name, the activation heights, the RPC address, cookie auth, the miner address, or the light server's RPC target and gRPC address. zrehearse rejects such a plan and names the setting. All other settings go to the container unchanged.

`previous` activates at height 2 and the upgrade under test at `upgrade.height`. Later upgrades never activate. Zebra checks the upgrade names and their order.

## What gets checked

1. Before activation, the chain tip has the branch ID of `previous`.
2. At the same tip, the upgrade is `pending` and the next block has its branch ID.
3. At the activation height, the upgrade is `active`.
4. `getblock` returns the activation block.
5. The chain grows by `blocks_after` blocks.
6. The funded key has a spendable block reward.
7. The coinbases before and at activation pay a shielded pool, for example `109 orchard 1, 110 ironwood 1`. This check runs when both blocks come after block 100.

With a light server, zrehearse starts it one block before activation and compares its gRPC answers with zebrad.

8. It serves the block before activation.
9. It serves the tip after activation.
10. `GetLightdInfo` reports the new branch ID.
11. `GetBlock` hashes agree with zebrad.
12. The compact blocks carry the shielded rewards.
13. `GetTreeState` agrees with `z_gettreestate`.
14. `GetSubtreeRoots` agrees with `z_getsubtreesbyindex`.

The gRPC calls use `fullstorydev/grpcurl:v1.9.3` and the lightwallet-protocol v0.5.0 protos in `proto/`.

Projects run only when all checks pass. Otherwise they are skipped.

## What your project gets

| Variable | Example |
|---|---|
| `ZREHEARSE_RPC_URL` | `http://127.0.0.1:32768`, zebrad JSON-RPC without auth |
| `ZREHEARSE_UPGRADE` | `NU6.3` |
| `ZREHEARSE_ACTIVATION_HEIGHT` | `110` |
| `ZREHEARSE_BRANCH_ID` | `37a5165b` |
| `ZREHEARSE_PREVIOUS_UPGRADE` | `NU6.2` |
| `ZREHEARSE_PREVIOUS_BRANCH_ID` | `5437f330` |
| `ZREHEARSE_TIP` | `120` |
| `ZREHEARSE_FUNDED_ADDRESS` | `tmV6ufuf8ERqa6nh5CdiyLyzvhXrpAojC7R` |
| `ZREHEARSE_FUNDED_KEY` | The WIF secret key of that address |
| `ZREHEARSE_SHIELDED_ADDRESS` | The Unified Address of the shielded rewards |
| `ZREHEARSE_SHIELDED_MNEMONIC` | Its seed phrase. Set only with the default address. |
| `ZREHEARSE_LIGHTWALLETD_URL` | `http://127.0.0.1:32895`, gRPC without TLS. Set only with a light server. |

Blocks 1 to 100 pay a new transparent key for each run. A block reward is spendable 100 blocks later, so at the default tip the rewards of blocks 1 to 21 are spendable. A project can sign a transaction with `ZREHEARSE_FUNDED_KEY` and send it with `sendrawtransaction`.

Later blocks pay the shielded address. The default address is account 0 of zrehearse's public test seed phrase (`hidden` x 23 + `protect`). Everyone can read it, so never use it for real funds. Set `funding.shielded_address` to use your own address. A wallet birthday must be 2 or more. A node without `generatetoaddress`, such as Zakura 1.6.0, gives no shielded funds, and a plan with its own address then fails with exit code 2.

`examples/spend.rs` is a sample project. The node must accept a spend that is signed for the new branch ID and reject spends that are signed for the previous one.

Exit code 0 means pass. Anything else, or a timeout, means fail.

## GitHub Action

```yaml
jobs:
  rehearse:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: RajeshRk18/zrehearse@v0
        with:
          plan: zrehearse/nu7.toml
```

The action needs a Linux runner, because GitHub's macOS runners have no Docker. `image` replaces `node.image` in the plan. `out` sets the output directory (default `zrehearse-out`), and the `report` output gives the path to `report.json`. With a release tag such as `@v0`, the action downloads the release binary. With any other ref, it builds zrehearse from that ref.

## Version matrix

To rehearse one plan on more than one Zebra version, run it once for each image. In GitHub Actions, use a matrix.

```yaml
jobs:
  rehearse:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        image: ["zfnd/zebra:6.2.3", "zfnd/zebra:7.0.0-rc.0"]
    steps:
      - uses: actions/checkout@v6
      - uses: RajeshRk18/zrehearse@v0
        with:
          plan: zrehearse/nu6_3.toml
          image: ${{ matrix.image }}
```

On your machine, use a loop.

```sh
for image in zfnd/zebra:6.2.3 zfnd/zebra:7.0.0-rc.0; do
  zrehearse run zrehearse/nu6_3.toml --image "$image" --out "out/${image##*:}"
done
```

Each image must know both upgrade names in the plan.

## Output

Everything goes to `out/<plan file name>/`, for example `out/nu6_3/` for `examples/nu6_3.toml`, or to the directory you pass with `--out`.

- `report.json` has every check and project result. Its `node` object has the container state, the zebrad exit code and, for a failed run, the error lines from the zebrad log.
- `zebrad.log` holds the last 300 lines of the node's log.
- `project-<name>.log` holds each project's stdout and stderr.
- `zebrad.toml` is the config the node ran with.
- `lightserver.log` holds the last 300 lines of the light server's log, and `report.json` has a `light_server` object like `node`.

When a run fails, zrehearse also prints the last zebrad error line.

The process exits with 0 when the rehearsal passed and 1 when a check or project failed. It exits with 2 when the rehearsal could not run. Examples are a missing Docker, an invalid plan, a zebrad that exits before its RPC answers, and an image that does not know `name` or `previous`. Then `setup_error` in `report.json` gives the cause.


## Rehearsing a new upgrade

A new upgrade needs no change to zrehearse. Point a plan at a Zebra image that knows the upgrade, and set `name` and `previous`.

## Limits

- The chain is a private regtest chain. zrehearse never connects to testnet or mainnet, so it does not replace a test on the real network with real peers and timing.
- No shadow forks. The chain starts empty, not from a copy of mainnet state. Zebra can carry Mainnet history as a configured Testnet with a moved activation height, but that needs a state of about 255 GiB. There are no plans for this. If it is ever added, it must be optional.
- No transaction generator. Blocks contain only their block reward, unless your suite sends transactions. There are no plans for a generator. If it is ever added, it must be optional.
- No wallet backend container such as Zallet.
- Your suite must read the node and light server endpoints from the `ZREHEARSE_*` environment variables. A suite with fixed ports, or one that only knows testnet, needs a small change.
- Windows. Projects run under `sh`.

## Development

The tests in `tests/rehearse.rs` run real rehearsals, so they need Docker and the node images `zfnd/zebra:6.2.3`, `zfnd/zebra:7.0.0-rc.0` and `zakuracore/zakura:1.6.0`. They rehearse the examples, the NU6.3 example on a second image, Zaino on NU6.3 and NU7, Zakura without a lockbox and with its own shielded address, a misspelled upgrade, an upgrade the image does not know and a container that fails to start. They also need `electriccoinco/lightwalletd:v0.5.4`, `zingodevops/zaino:0.10.1-no-tls` and `fullstorydev/grpcurl:v1.9.3`.

`docs/upstream-dependencies.md` lists the upstream interfaces that zrehearse depends on and how likely each one is to change.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                                # unit tests
cargo test --test rehearse -- --ignored   # real rehearsals
```
