# Model-less Execution Worker Protocol

## Goal

Make one `ExecutionSession` remotely addressable without introducing `Server`, `Agent`, providers, prompts, or autonomous scheduling.

## Boundary

One worker owns one existing workspace and one `ExecutionSession`.
The future supervisor owns worker creation, authentication, routing, idempotency journals, reconnection, and multi-session coordination.

## Protocol v1

Line-delimited JSON over stdin/stdout for the spike.
Every request and response carries `protocol: 1` and a caller-supplied stable `id`.

Commands:
- `inspect`
- `tool_list`
- `tool_call { tool, input }`
- `close`

Response:
- success: `{ protocol, id, ok: true, result }`
- failure: `{ protocol, id, ok: false, error }`

## Non-goals

No network listener, MCP, auth, command journal, process plane, background execution, worktree creation, model state, or session catalog.