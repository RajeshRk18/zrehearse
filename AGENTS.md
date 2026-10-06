# zrehearse

## Objective and scope

zrehearse is a Rust CLI and a GitHub Action. It starts a regtest Zebra node in
Docker, activates one network upgrade at a planned height, checks the
activation, and runs a downstream project's test command against the node.
It also funds a transparent key, so that projects can sign and send
transactions across the boundary.

Out of scope for now are light servers, version matrices, shadow forks and
Windows. See the README section "Not built yet".

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
- The Docker tests need `zfnd/zebra:6.2.3` and `zfnd/zebra:7.0.0-rc.0`.
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

## Guardrails

- `.github/workflows/ci.yml` is the single source of truth for the check
  commands. If you change a command, change the table above and the README
  "Development" section in the same commit.
- zrehearse has no list of upgrades. Zebra checks the upgrade names and their
  order. Do not add an upgrade table.
- `action.yml` must stay at the repo root.

## Conventions

- Commit messages are one line.
- In prose, do not join clauses with ':' or ';'.
