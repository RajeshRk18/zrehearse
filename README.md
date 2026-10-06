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
PASS  activation block is readable  (hash e653d57e...)
PASS  chain grows after activation  (tip 120, expected 120)
PASS  funded key has a spendable block reward  (21 spendable outputs for tmVyvHg46HFaiRbHuB5pVxNQ4Nj7YRjxdjc at tip 120)
PASS  project rpc-smoke  (exit 0, 0.2s, log out/nu6_3/project-rpc-smoke.log)
PASS  project spend  (exit 0, 0.6s, log out/nu6_3/project-spend.log)
REHEARSAL PASSED  report: out/nu6_3/report.json
```

`examples/nu7.toml` rehearses NU7 on `zfnd/zebra:7.0.0-rc.0`, the first Zebra image that knows NU7.

`examples/light_server.toml` adds lightwalletd in front of the node.

`examples/failing_project.toml` is the negative control. The node activates the upgrade fine, but its project exits with code 3, so the whole run fails.

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

zrehearse calls the `docker` CLI rather than the Docker API, because every machine that can run the rehearsal already has it. The container is removed when the run ends, including when it fails. Each container carries the label `zrehearse`. If you interrupt a run with Ctrl-C, clean up with `docker rm -f $(docker ps -aq --filter label=zrehearse)`.

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

| Field | Default | Meaning |
|---|---|---|
| `name` | required | Free text, copied into the report |
| `node.image` | required | Zebra image to run. The image must know both upgrade names. |
| `node.ready_timeout_secs` | `120` | How long to wait for zebrad's RPC to answer |
| `upgrade.name` | required | The upgrade under test, as Zebra names it, for example `NU6.3`. NU5 and later are supported. |
| `upgrade.previous` | required | The upgrade active before it, for example `NU6.2` |
| `upgrade.height` | `110` | Activation height, 3 or more. `height + blocks_after` must be above 100, so that the funded key has a spendable block reward. |
| `upgrade.blocks_after` | `10` | Blocks to mine after activation, before projects run |
| `project.name` | required | Letters, digits, `-` and `_`. It names the log file. |
| `project.run` | required | Shell command, run with `sh -c` from the plan file's directory |
| `project.timeout_secs` | `600` | The project fails if it runs longer. zrehearse then stops the `sh` process, but not the processes that `sh` started. |
| `light_server.kind` | none | `lightwalletd` or `zaino`. Leave out the `[light_server]` table to run without a light server. |
| `light_server.image` | required with `kind` | Light server image, for example `electriccoinco/lightwalletd:v0.5.4` or `zingodevops/zaino:0.10.1-no-tls`. Zaino needs a `-no-tls` tag. |
| `light_server.ready_timeout_secs` | `120` | How long the light server may take to serve the block at each tip |

The node config sets only two heights. `previous` activates at height 2, and the upgrade under test activates at your height. Zebra activates the upgrades before `previous` at height 2 or lower. Later upgrades are left out, so they never activate. zrehearse has no list of upgrades. Zebra checks the names and their order.

## What gets checked

zrehearse reads `getblockchaininfo` and compares it with the plan.

1. One block before the activation height, `consensus.chaintip` is the branch ID of `previous`.
2. At the same tip, the upgrade is `pending` at the planned height, and `consensus.nextblock` is its branch ID.
3. At the activation height, the upgrade is `active` and `consensus.chaintip` is its branch ID.
4. `getblock` can return the activation block.
5. After mining `blocks_after` more blocks, the tip is where it should be.
6. `getaddressutxos` shows a block reward of the funded key that the next block can spend.

With a light server, zrehearse starts it at one block before activation, so it sees the activation block arrive. Then it calls the light server over gRPC and compares the answers with zebrad.

7. One block before activation, the light server serves that block.
8. After activation, it serves the block at the tip.
9. `GetLightdInfo` reports the branch ID of the upgrade.
10. `GetBlock` at the activation height and at the tip gives the same hashes as zebrad.
11. `GetTreeState` at the same heights gives the same Sapling and Orchard trees as `z_gettreestate`.
12. `GetSubtreeRoots` streams to the end for Sapling and Orchard, with as many roots as `z_getsubtreesbyindex`.

zrehearse makes the gRPC calls with `fullstorydev/grpcurl:v1.9.3` in Docker. It uses the `.proto` files of lightwallet-protocol v0.5.0 in `proto/`, because Zaino has no gRPC reflection. The light server and grpcurl share zebrad's network namespace, so zrehearse creates no Docker network.

Projects run only when all checks pass. If the node never reached the planned state, a project failure would tell you nothing about your code, so projects are marked skipped instead.

## What your project gets

The project command runs with these environment variables.

| Variable | Example |
|---|---|
| `ZREHEARSE_RPC_URL` | `http://127.0.0.1:32768` (zebrad JSON-RPC, no auth) |
| `ZREHEARSE_UPGRADE` | `NU6.3` |
| `ZREHEARSE_ACTIVATION_HEIGHT` | `110` |
| `ZREHEARSE_BRANCH_ID` | `37a5165b`, as zebrad reports it |
| `ZREHEARSE_PREVIOUS_UPGRADE` | `NU6.2` |
| `ZREHEARSE_PREVIOUS_BRANCH_ID` | `5437f330`, as zebrad reports it |
| `ZREHEARSE_TIP` | `120` |
| `ZREHEARSE_FUNDED_ADDRESS` | `tmV6ufuf8ERqa6nh5CdiyLyzvhXrpAojC7R` |
| `ZREHEARSE_FUNDED_KEY` | The secret key of that address in WIF, for the compressed public key |
| `ZREHEARSE_LIGHTWALLETD_URL` | `http://127.0.0.1:32895`, the light server's gRPC endpoint without TLS. Set only with a `[light_server]`. |

zrehearse makes a new transparent key for each run, and every block pays its reward to that key. Zcash lets a block reward be spent only 100 blocks after its block. With the default heights, the next block can spend the rewards of blocks 1 to 21 when projects run, and all of them were mined before activation. Zebra on regtest permits a block reward to be spent to a transparent address. Thus a project can sign a transaction with its own code and send it with `sendrawtransaction`. `getaddressutxos` lists the outputs of the address. `report.json` gives the address as `funded_address`.

`examples/spend.rs` is a sample project of this kind. Both example plans run it. It signs three spends with its own ZIP 244 code. The node must accept the spend for the new branch ID. It must reject the spend for the previous branch ID, and also the spend with the new branch ID in its header but a sighash for the previous branch.

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
- Shadow forks. The chain starts empty, not from a copy of mainnet state. Zebra can carry Mainnet history as a configured Testnet with a moved activation height, but that needs a state of about 255 GiB.
- A transaction generator that sends every transaction type across the activation height.
- No wallet backend container such as Zallet.
- zrehearse funds one transparent key. A suite that needs shielded funds must shield them first.
- Your suite must read the node and light server endpoints from the `ZREHEARSE_*` environment variables. A suite with fixed ports, or one that only knows testnet, needs a small change.
- Windows. Projects run under `sh`.

## Development

The tests in `tests/rehearse.rs` run real rehearsals, so they need Docker and the Zebra images `zfnd/zebra:6.2.3` and `zfnd/zebra:7.0.0-rc.0`. They rehearse the examples, the NU6.3 example on a second image, Zaino on NU6.3 and NU7, a misspelled upgrade, an upgrade the image does not know and a container that fails to start. They also need `electriccoinco/lightwalletd:v0.5.4`, `zingodevops/zaino:0.10.1-no-tls` and `fullstorydev/grpcurl:v1.9.3`.

`docs/upstream-dependencies.md` lists the upstream interfaces that zrehearse depends on and how likely each one is to change.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                                # unit tests
cargo test --test rehearse -- --ignored   # real rehearsals
```
