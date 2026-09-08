#!/usr/bin/env bash
set -euo pipefail

export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
export PATH="/usr/local/cargo/bin:$PATH"

INPUT=${INPUT_ARCHIVE:-/mnt/artifacts/project-executor-source.tar.zst}
OUT=${OUTPUT_DIR:-/mnt/artifacts/project-executor-build}
SRC=/work/source

rm -rf "$SRC"
mkdir -p "$SRC" "$OUT"
tar --zstd --no-same-owner -xf "$INPUT" -C "$SRC"
cd "$SRC"

rustc --version | tee "$OUT/rustc.txt"
cargo --version | tee "$OUT/cargo.txt"
sha256sum Cargo.lock | tee "$OUT/Cargo.lock.sha256"
sha256sum "$INPUT" | tee "$OUT/source-archive.sha256"

cargo test --locked -p jcode-project-executor --no-run --message-format=json \
  | tee "$OUT/cargo-test-build.jsonl" >/dev/null
jq -r 'select(.reason == "compiler-artifact" and .profile.test == true and .executable != null) | .executable' \
  "$OUT/cargo-test-build.jsonl" \
  | sed "s#^$SRC/##" \
  | sort -u > "$OUT/test-executables.txt"

test -s "$OUT/test-executables.txt"
cargo build --locked -p jcode-project-executor --bins
{
  cat "$OUT/test-executables.txt"
  printf "%s\n" \
    target/debug/jcode-project-executor-supervisor \
    target/debug/jcode-project-executor-worker
} | tar --zstd -cf "$OUT/acceptance-bundle.tar.zst" -T -

sha256sum "$OUT/acceptance-bundle.tar.zst" \
  target/debug/jcode-project-executor-supervisor \
  target/debug/jcode-project-executor-worker \
  | tee "$OUT/artifacts.sha256"
