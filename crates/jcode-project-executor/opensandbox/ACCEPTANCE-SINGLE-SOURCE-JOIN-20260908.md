# Project Executor single-source join acceptance — 2026-09-08

**Authority effect:** `NONE`. This record establishes source/verification evidence only. It does not authorize runtime mutation, deployment, retirement, or whole-system acceptance.

## Joined source identity

- Branch: `work/project-executor-single-source-join-20260908`
- Foundation: `d56fcd315cd192297990e846e532fda33cc2ecf6`
- Accepted/live production implementation ancestor: `fd8d10365b55a6b0261707c75843db91f2b29108`
- Prior acceptance evidence head: `45b4e697c8c2c50919cb0af245f761dbce6607b1`
- Joined implementation/test head: `1e024cc283db8932f184b7a9c48d9465c0965d30`
- Original verifier-fix commits: `52159e5b3d84443a724021c2b8250bd3d5116a34` and `05bb21c1517e986e920cdfd6155de6de33e35e2e`
- Cherry-picked equivalents: `90c0907f1...` and `1e024cc28...`

Stable patch IDs prove exact verifier-fix preservation:

- `52159e5...` = `90c0907...` -> `5351c678a62bc78a4fc64b1e9fde4dda42690dc7`
- `05bb21c...` = `1e024cc...` -> `a01143e0d8473461f9ee7b58eb6f7a63c1d6bb69`

## Reproducible build

Qualified builder: CT102 / Debian 12 Bookworm, image `local/project-executor-builder:bookworm-rust1.96-v2`.

- rustc: `1.96.1 (31fca3adb 2026-06-26)`
- cargo: `1.96.1 (356927216 2026-06-26)`
- Cargo.lock SHA-256: `88a1faa3d7b3f65c6c1e6f5f6176c4d48df5e9fbb53b2e564c18bb7d52166bce`
- Joined source archive SHA-256: `a491332c50fe143920d3cdc1e1732fef82613cd256b2a130b6e3e84a2e30e0c1`
- Acceptance bundle SHA-256: `99e69875ffaa09bb9739443b0b0163212fb3851fa6d1e4d6a0c1ee2671a003f6`
- Supervisor SHA-256: `35d8fc587d3174819b456de87e323f96d894fdfb3f2a933caae98a0681496ca9`
- Worker SHA-256: `9c075ac514c98b20472ca7c648c92ef95d063bc2d22b407b6dcdd21908cbf4d8`

The supervisor and worker hashes are byte-identical to the previously accepted `fd8d103...` build. The join changes verifier/test code only; accepted production behavior is preserved.

## Qualified CT5011 replay

CT5011 is Debian 12 Bookworm with bubblewrap `0.8.0`. No Rust toolchain or package changes were introduced. The CT102-built acceptance bundle was transferred by digest and unpacked without changing CT5011's security ceiling.

Targeted parallel verification (`--test-threads=14`):

- `transactional_upgrade_supervisor_adversarial`: **14/14 PASS**
- `retirement_api_red`: **16/16 PASS**

Complete compiled Project Executor suite:

- parallel replay: **23/23 executables, 131/131 tests PASS, 0 failed, 0 ignored**
- serial replay: **23/23 executables, 131/131 tests PASS, 0 failed, 0 ignored**

A direct PVE run is not acceptance evidence because PVE lacks `/usr/bin/bwrap`; the observed failures occurred at sandbox construction before test semantics and are classified `INVALID_VERIFIER_ENVIRONMENT`, not product failures.

## Disposition

`1e024cc283db8932f184b7a9c48d9465c0965d30` is the verified single-source implementation/test successor joining the accepted `fd8d103...` production lineage with the later verifier-race corrections. This evidence commit may advance the branch without changing the implementation identity recorded above.
