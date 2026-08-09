# jcode-derived project executor provenance

`jcode-project-executor` is a model-less extraction, not the jcode agent runtime.

Donor evidence:

- `3d6a217d4b3cf012d5ce4af4205bf1dd656fc808` — provider-free `ExecutionSession`, deterministic execution-worker baseline, and process-group containment for timed-out execution-session commands.
- `42b5ad2cf0afffcab17f6415b8cd48ac8b7c70e5` — owned long-lived process manager with file-backed output and process-group cleanup.

The extracted implementation intentionally changes the donor interface:

- process start is argv-only, never a shell command string;
- `process.wait` is explicit and bounded when a timeout is supplied;
- cwd is constrained to the owned workspace;
- the crate has no dependency on jcode agent, provider, prompt, TUI, swarm, or model crates.

The Harness `project-executor/v1` contract remains authoritative. Donor commit IDs are provenance only and are not protocol fields.
