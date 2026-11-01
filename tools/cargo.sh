#!/usr/bin/env bash
set -euo pipefail
# Prefer the project-local toolchain if this machine's system Rust is older.
task_toolchain="${CAPRA_RUST_TOOLCHAIN:-${XDG_CACHE_HOME:-$HOME/.cache}/capra/toolchains/1.95.0}"
if [[ -x "$task_toolchain/bin/cargo" ]]; then
  export PATH="$task_toolchain/bin:$PATH"
fi
exec cargo "$@"
