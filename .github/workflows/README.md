# Workflows

| Workflow | Runs on | What it does |
|---|---|---|
| `ci.yml` | every PR and push to `main` | `Format`: `cargo fmt --check`. `Clippy · test · WASM build`: clippy (warnings are errors), `cargo test` and a release WASM build. |
| `extend-ttls.yml` | schedule / manual | Extends storage TTLs of deployed contracts so they do not expire. Not a CI check. |

## Which contracts CI covers

`ci.yml` lists the crates it checks in `CRATES` (tests and WASM build),
`FMT_CRATES` (formatting) and `CLIPPY_CRATES` (clippy). These are the
contracts that currently compile and pass their tests.

The remaining workspace members do not compile or have failing tests yet,
mostly because a few shared crates are broken (for example
`provider-registry/src/lib.rs` has a syntax error left by a merge, and many
crates use it in their tests). When you fix one, add it to the lists in
`ci.yml` in the same PR so it stays fixed.

## Deploying

Deployment is not part of CI. Deploy from a machine that has the deployer
key, with `scripts/deploy_testnet.sh` or `scripts/deploy_all.sh`. Check WASM
sizes locally with `scripts/check-wasm-sizes.sh`.

## PR template

Every PR must confirm the checkbox: "This PR changes only the code
needed for the referenced issue."
