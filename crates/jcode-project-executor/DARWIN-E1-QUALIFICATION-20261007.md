# Project Executor Darwin E1 Qualification — 2026-10-07

State: **E1_NATIVE_MACDOM_PROVIDER_QUALIFIED_CANDIDATE**
Authority effect: **NONE**
Canonical promotion: **NOT PERFORMED**
Harness binding: **NOT PERFORMED**

## Source basis

Accepted Project Executor implementation/test revision:

`1e024cc283db8932f184b7a9c48d9465c0965d30`

This candidate preserves the existing Project Executor custody, effect-authorization,
receipt, recovery, retirement, and process lifecycle contracts and adds a bounded
Darwin process sandbox alongside the existing Linux bubblewrap backend.

## Darwin bounded-process backend

The macOS backend uses `/usr/bin/sandbox-exec` with:

- default deny;
- sanitized environment;
- explicit `PWD` bound to the validated cwd;
- admitted workspace read/write according to access mode;
- admitted Git common-directory access according to access mode;
- sensitive user/volume/temp roots denied outside admitted paths;
- metadata-only ancestor traversal required for canonical path resolution;
- `/dev/null` as the only explicitly admitted external write device;
- Command Line Tools Git selected directly to avoid `xcrun` temp-cache writes;
- existing process-group timeout/abort/descendant termination semantics retained.

Darwin-specific adversarial tests prove:

- outside writes are denied;
- symlink read escape is denied;
- host environment variables are scrubbed;
- outside paths remain non-observable to the bounded process test;
- linked-worktree Git remains usable through the admitted Git metadata seam.

## Darwin portability repairs outside the process backend

Two test/inspection portability issues were corrected without changing authority semantics:

- Unix owner inspection no longer assumes GNU `stat -c`; it uses file UID plus `id -nu`.
- retirement/restart fault-injection fixtures preserve the invoked `/var/...` path spelling on macOS rather than mixing it with canonical `/private/var/...` spelling.

## Verification

Executed on MacDom, macOS 26.6.2:

`cargo fmt --check`

PASS.

`git diff --check`

PASS.

`cargo test -p jcode-project-executor`

**134 passed / 0 failed.**

The process-manager subset is **11/11 PASS**, including the Darwin adversarial cases.

## Qualification boundary

This establishes an isolated, non-authoritative E1 candidate only.

It does **not** establish:

- Registry canonical source identity;
- Harness runtime binding;
- Operator -> Harness -> Executor -> MacDom E2E qualification;
- runtime restart/recovery qualification as a deployed service;
- DesktopCommander fallback conformance;
- P4, P5, P6, or mutation authority.

Next dependency: **E2 — bind the current Harness `project-executor/v1` seam to this exact candidate/runtime identity without promoting Registry authority.**
