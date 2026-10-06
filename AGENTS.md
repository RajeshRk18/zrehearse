# zrehearse

## Objective and scope

zrehearse is a Rust CLI and a GitHub Action. It starts a regtest Zebra node in
Docker, activates one network upgrade at a planned height, checks the
activation, and runs a downstream project's test command against the node.
It also funds a transparent key, so that projects can sign and send
transactions across the boundary, and it can put lightwalletd or Zaino in
front of the node and check what they serve over gRPC.

Out of scope are shadow forks, a transaction generator and Windows. If shadow
forks or a generator are ever added, they must be optional. See the README
section "Limits".

## Commands

| Task | Command |
|---|---|
| Format | `cargo fmt --check` |
| Lint | `cargo clippy --all-targets -- -D warnings` |
| Unit tests | `cargo test` |
| Docker tests | `cargo test --test rehearse -- --ignored` |
| Example run | `cargo run -- run examples/nu6_3.toml` |

## Prerequisites

- Docker must be running.
- The Docker tests need `zfnd/zebra:6.2.3`, `zfnd/zebra:7.0.0-rc.0`,
  `electriccoinco/lightwalletd:v0.5.4`, `zingodevops/zaino:0.10.1-no-tls` and
  `fullstorydev/grpcurl:v1.9.3`. The light server images are amd64 only.
  Docker runs them on arm64 hosts with emulation.
- The Rust toolchain must support edition 2024.

## Known failure patterns

- A run leaves a container after Ctrl-C. Remove it with
  `docker rm -f $(docker ps -aq --filter label=zrehearse)`.
- A plan with `height + blocks_after` of 100 or less is rejected. A block
  reward is spendable only 100 blocks later.
- zebrad can exit before Docker reports its port. `Node::start` then returns
  the node, and `wait_ready` reports the exit. Do not change this back to an
  error at port lookup.
- zcash_protocol 0.10.6 has NU7 only as the placeholder `0xffffffff`. Thus
  `examples/spend.rs` computes the ZIP 244 sighash itself. Do not replace it
  with the zcash crates until they have the real NU7 branch ID `77190ad9`.
- Zebra 6.2.3 mines about 0.12 s per block, so one rehearsal takes about 20 s.
- Docker can run out of address pools for new networks on hosts with many
  networks. Thus zrehearse creates no network. The light server and grpcurl
  join zebrad's network namespace with `--network container:<zebrad>`.
- Zaino answers `GetLightdInfo` with zebrad's height before its own index has
  the block. Only `GetBlock` at the height proves that it serves the block.
- Zaino 0.10.1 cannot serve an NU7 chain. Its index stops at the block before
  activation. The test `zaino_0_10_1_stops_at_nu7` expects this.
- Zaino colors its log. `docker::error_lines` strips the color codes.

## Guardrails

- `.github/workflows/ci.yml` is the single source of truth for the check
  commands. If you change a command, change the table above and the README
  "Development" section in the same commit.
- zrehearse has no list of upgrades. Zebra checks the upgrade names and their
  order. Do not add an upgrade table.
- `action.yml` must stay at the repo root.
- When code starts or stops depending on an upstream image, flag, env var or
  RPC, update `docs/upstream-dependencies.md` in the same commit.

## Conventions

- Commit messages are one line.
- In prose, do not join clauses with ':' or ';'.
