#!/usr/bin/env bash
set -euo pipefail

APP_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOOM_SRC="${LOOM_SRC:-$(dirname "$APP_ROOT")/loom}"
export WEAVER_HOME="${WEAVER_HOME:-$APP_ROOT/.local/loom-state}"
export WEAVER_API=http://127.0.0.1:7878

# The project-local prerequisites are optional; normal system installs work too.
if [ -x "$APP_ROOT/.local/node/node_modules/.bin/node" ]; then
  export PATH="$APP_ROOT/.local/node/node_modules/.bin:$PATH"
fi
if [ -d "$APP_ROOT/.local/agents/node_modules/.bin" ]; then
  export PATH="$APP_ROOT/.local/agents/node_modules/.bin:$PATH"
fi
if [ -d "$APP_ROOT/.local/sysroot/usr/lib/x86_64-linux-gnu/pkgconfig" ]; then
  LOCAL_LIB="$APP_ROOT/.local/sysroot/usr/lib/x86_64-linux-gnu"
  export PKG_CONFIG_PATH="$LOCAL_LIB/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  export LD_LIBRARY_PATH="$LOCAL_LIB${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  export WEBKIT_EXEC_PATH="$LOCAL_LIB/webkit2gtk-4.1"
fi

cd "$APP_ROOT"
LOOM_PID=""
cleanup() {
  if [ -n "$LOOM_PID" ]; then
    kill "$LOOM_PID" 2>/dev/null || true
    wait "$LOOM_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if ! curl --silent --fail --max-time 2 "$WEAVER_API/api/health" >/dev/null; then
  if [ ! -f "$LOOM_SRC/Cargo.toml" ]; then
    echo "Loom checkout missing: $LOOM_SRC. Clone https://github.com/marin-community/loom there or set LOOM_SRC." >&2
    exit 1
  fi
  cargo build --manifest-path "$LOOM_SRC/Cargo.toml" -p loom -p tapestry --locked
  export PATH="$LOOM_SRC/target/debug:$PATH"
  # Reuse the authenticated GitHub identity to seed this local instance.
  if [ -z "${LOOM_OWNER_GITHUB:-}" ]; then
    export LOOM_OWNER_GITHUB="$(gh api user --jq .login)"
  fi
  if [ -z "${LOOM_OWNER_GITHUB_ID:-}" ]; then
    export LOOM_OWNER_GITHUB_ID="$(gh api "users/$LOOM_OWNER_GITHUB" --jq .id)"
  fi
  mkdir -p "$WEAVER_HOME"
  echo "Starting Loom at $WEAVER_API (state: $WEAVER_HOME)"
  "$LOOM_SRC/target/debug/loom" server run --addr 127.0.0.1:7878 >"$WEAVER_HOME/server.log" 2>&1 &
  LOOM_PID=$!
  ready=false
  for attempt in {1..60}; do
    if curl --silent --fail --max-time 2 "$WEAVER_API/api/health" >/dev/null; then
      ready=true
      break
    fi
    if ! kill -0 "$LOOM_PID" 2>/dev/null; then
      break
    fi
    sleep 1
  done
  if [ "$ready" != true ]; then
    tail -n 40 "$WEAVER_HOME/server.log" >&2
    exit 1
  fi
else
  echo "Using Loom already running at $WEAVER_API"
fi

npx --no-install tauri dev
