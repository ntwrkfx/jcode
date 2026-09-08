# Project Executor reproducible build acceptance — 2026-09-08

Authority effect: **NONE**. This records build and behavioral evidence; it does not authorize deployment or promotion.

## Accepted implementation

- Implementation commit: `fd8d10365b55a6b0261707c75843db91f2b29108`
- Base commit: `d56fcd315cd192297990e846e532fda33cc2ecf6`
- Branch: `work/hc-executor-reproducible-20260908`
- Normalized source archive SHA-256: `4bf41a878c1c308e749e0275017aad7cb824d89e54f06555f7b7993491fd113a`
- Repacked committed tree SHA-256: `4bf41a878c1c308e749e0275017aad7cb824d89e54f06555f7b7993491fd113a`
- Source identity result: **EXACT MATCH**

## Builder

- OpenSandbox host: CT102 / Debian 12 Bookworm
- Profile image: `local/project-executor-builder:bookworm-rust1.96-v2`
- Image SHA-256: `a734b4b1bc6573b356b966e677d40491a74adb4cff3d013b5043899552363c73`
- Rust: `rustc 1.96.1 (31fca3adb 2026-06-26)`
- Cargo: `cargo 1.96.1 (356927216 2026-06-26)`
- `Cargo.lock` SHA-256: `88a1faa3d7b3f65c6c1e6f5f6176c4d48df5e9fbb53b2e564c18bb7d52166bce`

## Build artifacts

- Acceptance bundle SHA-256: `00a1c77f68dbbfdf4859fc25513fbca1c0f6c60576b52b196d650e16c3ee8442`
- Supervisor SHA-256: `35d8fc587d3174819b456de87e323f96d894fdfb3f2a933caae98a0681496ca9`
- Worker SHA-256: `9c075ac514c98b20472ca7c648c92ef95d063bc2d22b407b6dcdd21908cbf4d8`
- OpenSandbox compiled all Project Executor tests with `cargo test --locked --no-run`.

## CT5011 behavioral acceptance

The OpenSandbox security ceiling intentionally blocks nested `bwrap`; it was not weakened. The exact Bookworm-built acceptance bundle was therefore replayed in CT5011, where the production Project Executor sandbox dependency is available.

- Compiled test executables: **23 / 23 PASS**
- Individual Rust tests: **131 PASS / 0 FAIL / 0 ignored**
- Independent stdout/stderr cursor regression: **PASS under real `bwrap`**
- Replay mode: serialized test executables, `TMPDIR=/tmp`
- No runtime service restart, deployment, or canonical promotion occurred.

The previously deployed runtime identity `8c48d362c4d04bac3d56094fb0d02d96380d306f` remains historical behavioral evidence only; its source provenance is unresolved and it is not the accepted source identity.

## HC-07 bounded direct canary

A source-built supervisor from implementation commit `fd8d10365b55a6b0261707c75843db91f2b29108` was started on an isolated CT5011 socket, session root, worktree root, and device UUID. The canonical Executor and route were not changed.

- Health: `SERVING` / recovery `HEALTHY`
- Session create: `READY`
- Process exit: `0`
- stdout cursor: `0 -> 3 -> 6`, data `abc` then `def`
- stderr cursor: `0 -> 3 -> 5`, data `123` then `45`
- stdout/stderr EOF: true on final independent reads
- Session close: clean; receipt state `CLOSED`
- Reported implementation revision: `fd8d10365b55a6b0261707c75843db91f2b29108`
- Canary supervisor was stopped after acceptance.
- Existing `8c48d362...` supervisor PID and route listener on `10.0.0.191:9443` remained unchanged.
