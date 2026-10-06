#!/usr/bin/env bash
# Rebuild Arachne and prepare the .app bundle for copying to /Applications.
# Usage: scripts/rebuild.sh [--install]
#   --install  also copy the app to /Applications (may prompt for sudo)

set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Arachne"
APP_BUNDLE="src-tauri/target/release/bundle/macos/${APP_NAME}.app"

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

# 1. Dependencies
if [[ ! -d node_modules ]]; then
  say "Installing npm dependencies"
  npm install
fi

# 2. Typecheck + tests (fast, catches breakage before the slow build)
say "Typechecking"
npm run typecheck

say "Running tests"
npm test

# 3. Build frontend + Rust, bundling only the .app (skip DMG for speed)
say "Building release bundle"
npx tauri build --bundles app

if [[ ! -d "$APP_BUNDLE" ]]; then
  echo "error: expected bundle at $APP_BUNDLE but it wasn't created" >&2
  exit 1
fi

say "Bundle ready: $APP_BUNDLE"

# 4. Optionally install
if [[ "${1:-}" == "--install" ]]; then
  say "Copying to /Applications"
  rm -rf "/Applications/${APP_NAME}.app"
  cp -R "$APP_BUNDLE" /Applications/
  say "Installed. Launch with: open -a ${APP_NAME}"
else
  say "Done. To install: cp -R \"$APP_BUNDLE\" /Applications/"
fi
