# Minimal ExecutionSession Plan

**Goal:** Add the smallest remotely addressable execution-session primitive above `Registry::execution()` without using jcode `Server`, `Agent`, providers, swarm state, ambient scheduling, or MCP pools.

## Contract

`ExecutionSession` binds one existing workspace directory and owns:

- stable session id;
- canonical workspace path;
- provider-free execution registry;
- monotonically unique tool-call ids;
- open/closed lifecycle state.

Initial operations:

- `create(workspace)`;
- `inspect()`;
- `tool_names()`;
- `call_tool(name, input)`;
- `close()`.

## Safety boundary

- `ToolExecutionMode::Direct` only.
- No stdin channel or graceful agent shutdown signal.
- Explicit `bash` calls with `run_in_background=true` are rejected until a session-owned process plane exists.
- Foreground bash timeout promotion in the inherited tool remains known technical debt; do not claim process ownership or complete cleanup until replaced or controlled.
- Session close does not yet terminate external processes.

## TDD sequence

1. Add tests for workspace binding/tool dispatch and closed-session rejection; verify RED because `ExecutionSession` does not exist.
2. Implement only lifecycle/context/tool dispatch.
3. Add and verify explicit background-bash rejection.
4. Run focused session tests plus the provider-free registry test.
5. Commit before designing process streaming or transport.

## Non-goals

No MCP server, worktree manager, event stream, PTY, process read/write API, remote transport, persistence, or jcode `Server` integration in this task.
