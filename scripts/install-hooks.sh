#!/bin/sh
# Installs the arbx git hooks into .git/hooks/
# Run once after cloning: sh scripts/install-hooks.sh

set -e

REPO_ROOT="$(git rev-parse --show-toplevel)"

echo "==> Installing git hooks for arbx..."

cp "$REPO_ROOT/scripts/pre-commit" "$REPO_ROOT/.git/hooks/pre-commit"
chmod +x "$REPO_ROOT/.git/hooks/pre-commit"

echo "Git hooks installed."
echo "  pre-commit -> .git/hooks/pre-commit"
