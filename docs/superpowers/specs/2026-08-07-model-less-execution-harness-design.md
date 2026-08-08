# Model-Less Execution Harness Spike

## Goal

Prove the smallest jcode-derived execution substrate that can be driven externally without any model/provider decision path.

## Pinned source

- Repository: `https://github.com/1jehuang/jcode.git`
- Branch inspected: `master`
- Base SHA: `dd8755f7e71f0673911d481b625b8a559c81a8b6`
- Upstream version: `0.71.1`
- Worktree branch: `experiment/model-less-execution-harness`

## Verified seams

- `src/bin/harness.rs` already executes deterministic tools with a `NoopProvider`.
- `Registry::new(_provider)` currently does not use its provider argument.
- Deterministic tool implementations live under `crates/jcode-app-core/src/tool/`.
- `ToolContext.working_dir` provides a per-call workspace root.
- Current `Server` is not a clean extraction boundary: it owns `Provider`, `Agent` sessions, swarm state, ambient scheduling, MCP pooling, and model-related state.
- `jcode-harness-api` is a useful protocol donor, but its mutation/execution path is still agent/model oriented.

## Spike design

1. Add a provider-free execution registry constructor containing only deterministic execution tools needed for the smoke harness.
2. Prove the constructor through tests before changing the harness binary.
3. Switch `jcode-harness` from `NoopProvider + Registry::new(provider)` to the provider-free constructor and delete `NoopProvider` from that binary.
4. Keep the existing agent `Server` untouched during this spike.
5. If the provider-free harness passes, introduce a minimal `ExecutionSession` around `working_dir`, registry, cancellation/process ownership, and event/receipt state in a later task.

## Initial execution tool set

`read`, `write`, `edit`, `multiedit`, `patch`, `apply_patch`, `ls`, `bash`, and `batch`.

Network tools, browser, memory, swarm, session search, integrations, skills, Gmail, schedules, selfdev, and model/conversation functions are excluded.

## Success criteria

- The deterministic harness compiles without constructing or importing a `Provider`.
- Its existing filesystem/edit/shell smoke cases pass.
- The execution registry does not expose agent/network/integration tools.
- Installed jcode and live OMP processes remain untouched.
- No source change is made to the existing agent server in this spike.

## Stop condition

If extracting the deterministic registry requires importing provider/model runtime for reasons other than existing type residue, stop and reconsider the boundary instead of widening the spike.
