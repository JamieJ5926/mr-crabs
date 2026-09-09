#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

echo "== actions =="
sed -n '/pub enum AppAction/,/^}/p' crates/mr-crabs-app/src/action.rs \
  | grep -oE '^\s{4}[A-Z][A-Za-z0-9]*' | tr -d ' ' | sort

echo "== config-keys =="
grep -oE '"[a-z0-9-]+"' crates/mr-crabs-config/src/lib.rs \
  | tr -d '"' | grep -E '^[a-z]+(-[a-z0-9]+)+$' | sort -u

echo "== keybindings =="
sed -n '/fn default_keybindings/,/^}/p' crates/mr-crabs-app/src/keymap.rs \
  | grep -oE 'AppAction::[A-Za-z0-9]+' | sort

echo "== cli-flags =="
grep -oE '"--[a-z0-9-]+"|"\+[a-z0-9-]+"' crates/mr-crabs-app/src/bin/mr-crabs.rs \
  | tr -d '"' | sort -u
