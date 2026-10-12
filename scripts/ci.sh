#!/usr/bin/env bash
# Runs every check. Exit 0 only if all pass; exit 1 if any fails.
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

# Toolchain discovery: rustup installs cargo into ~/.cargo/bin, which is only
# on the PATH of login shells. In non-login contexts (cron, CI harnesses, GUI
# terminals) source the rustup bootstrap so the checks below can run at all.
if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "${HOME:-}/.cargo/env" ]; then
        # shellcheck disable=SC1091,SC1090
        . "${HOME}/.cargo/env"
    elif [ -x "${HOME:-}/.cargo/bin/cargo" ]; then
        PATH="${HOME}/.cargo/bin:${PATH}"
        export PATH
    fi
fi

failed=0

run() {
    echo "==> $*"
    if ! "$@"; then
        echo "FAILED: $*" >&2
        failed=1
    fi
}

run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
run cargo test --workspace --all-features --locked
RUSTDOCFLAGS="-D warnings" run cargo doc --workspace --all-features --no-deps --locked

if [ "$failed" -ne 0 ]; then
    echo "CI FAILED" >&2
    exit 1
fi
echo "CI OK"
