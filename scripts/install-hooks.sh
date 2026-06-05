#!/usr/bin/env bash
# Configure git to use .githooks/ for all hook scripts.
# Run once per clone: ./scripts/install-hooks.sh

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_DIR="${REPO_ROOT}/.githooks"

if [ ! -d "$HOOKS_DIR" ]; then
    echo "ERROR: .githooks/ directory not found at $HOOKS_DIR"
    exit 1
fi

git config core.hooksPath "$HOOKS_DIR"
echo "Git hooks configured: core.hooksPath = $HOOKS_DIR"
echo "Pre-commit hook will run: fmt check, clippy, unit tests"
