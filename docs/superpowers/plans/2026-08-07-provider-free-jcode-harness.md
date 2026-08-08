# Provider-Free Jcode Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make jcode's deterministic tool harness structurally provider-free while preserving its read/edit/shell smoke behavior.

**Architecture:** Add a curated `Registry::execution()` constructor that registers only deterministic execution tools and requires no provider. Keep the agent-facing `Registry::new(provider)` and `Server` unchanged. Then switch `jcode-harness` to the new constructor and remove its `NoopProvider` shim.

**Tech Stack:** Rust 2024 edition, Tokio, existing `jcode-app-core` tool implementations and tests.

## Global Constraints

- Base SHA: `dd8755f7e71f0673911d481b625b8a559c81a8b6`.
- No LLM/provider invocation path in `jcode-harness`.
- Do not modify the installed `~/.local/bin/jcode` release or live OMP processes.
- Do not modify the existing agent `Server` in this spike.
- Execution tool allowlist: `read`, `write`, `edit`, `multiedit`, `patch`, `apply_patch`, `ls`, `bash`, `batch`.
- Network, browser, swarm, memory, integration, scheduling, skills and conversation/model tools remain excluded.

---

### Task 1: Provider-Free Execution Registry

**Files:**
- Modify: `crates/jcode-app-core/src/tool/mod.rs`
- Test: `crates/jcode-app-core/src/tool/tests.rs`

**Interfaces:**
- Produces: `pub async fn Registry::execution() -> Registry`
- Preserves: existing `Registry::new(Arc<dyn Provider>)` behavior for agent callers.

- [ ] **Step 1: Write the failing registry test**

Add an async test that calls `Registry::execution()`, obtains definitions, and asserts the exact allowlist above. Also assert representative exclusions such as `webfetch`, `browser`, `swarm`, `memory`, `skill_manage`, and `conversation_search`.

- [ ] **Step 2: Run the focused test and verify RED**

Run:
```bash
cargo test -p jcode-app-core execution_registry_contains_only_deterministic_tools -- --nocapture
```
Expected: compile failure because `Registry::execution` does not yet exist.

- [ ] **Step 3: Implement the minimal constructor**

Create `Registry::execution()` with a default/non-shared `SkillRegistry`, fresh `CompactionManager`, and an explicit tool map containing only the allowlisted tools. Add `batch` after the registry exists so it receives the same shared tool map.

- [ ] **Step 4: Run focused test and verify GREEN**

Run the same focused test. Expected: PASS.

- [ ] **Step 5: Run adjacent tool-registry tests**

Run:
```bash
cargo test -p jcode-app-core tool::tests -- --nocapture
```
Expected: existing registry behavior remains green.

- [ ] **Step 6: Commit Task 1**

```bash
git add crates/jcode-app-core/src/tool/mod.rs crates/jcode-app-core/src/tool/tests.rs
git commit -m "feat: add provider-free execution registry"
```

### Task 2: Remove Provider From Deterministic Harness

**Files:**
- Modify: `src/bin/harness.rs`

**Interfaces:**
- Consumes: `Registry::execution()` from Task 1.
- Produces: `jcode-harness` binary with no `Provider`, `EventStream`, `Message`, or `ToolDefinition` imports.

- [ ] **Step 1: Establish the failing structural expectation**

Before changing production code, verify the harness still contains `NoopProvider` and `Registry::new(provider)`:
```bash
rg -n "NoopProvider|Registry::new\(provider\)|EventStream|Provider" src/bin/harness.rs
```
Expected: matches are present.

- [ ] **Step 2: Switch the harness to the execution registry**

Delete `NoopProvider` and its provider-specific imports. Replace provider construction plus `Registry::new(provider).await` with `Registry::execution().await`. Remove the network-backed harness option and cases so the binary's advertised scope matches the deterministic allowlist.

- [ ] **Step 3: Verify provider references are gone**

Run:
```bash
! rg -n "NoopProvider|Registry::new\(provider\)|EventStream|Provider" src/bin/harness.rs
```
Expected: exit 0 because none of the forbidden provider references remain.

- [ ] **Step 4: Build and run the smoke harness**

Run the binary against a temporary workspace and verify write/read/edit/patch/ls/bash/batch all execute successfully and no model credentials are required.

- [ ] **Step 5: Commit Task 2**

```bash
git add src/bin/harness.rs
git commit -m "refactor: make deterministic harness provider free"
```
