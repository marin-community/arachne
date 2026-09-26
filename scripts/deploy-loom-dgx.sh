#!/usr/bin/env bash
# deploy-loom-dgx.sh — install Loom natively on the DGX Spark as a systemd service.
#
# Arachne Phase 0, step 3: personal, Tailscale-only loom on the DGX.
#
# Strategy (deliberately NOT deploy/standalone's Docker+Caddy+domain stack —
# that exists for internet-facing team deploys with GitHub OAuth/App):
#   - loom is two self-contained binaries: `loom` + `tapestry`.
#   - Cross-compile on the Mac (arm64 → aarch64-linux), copy binaries over,
#     run under systemd. The Mac is faster than the DGX, and this keeps
#     worktrees as plain host paths under ~dlwh — which is exactly what Zed
#     remote development opens over SSH (Arachne's "Open in Zed" button).
#   - LOOM_RUNNER=local (the default): session supervisors run as processes on
#     the host, worktrees under ~/.weaver/repos. No Docker socket games.
#   - GitHub auth without a GitHub App/OAuth app:
#       - loom's "Account PAT" (auth.github_token.set) seeded from the DGX's
#         existing gh login → interactive sessions git-push / gh as dlwh;
#       - global git insteadOf rewrite so loom's plain-https managed-repo
#         clones authenticate via gh's credential helper.
#   - The on-DGX loom CLI authenticates via the machine-local token
#     (~/.weaver/loom-token, 0600) — minted automatically on first boot.
#   - Remote access from the Mac: personal API token printed once below.
#
# Idempotent: safe to re-run. Re-deploys swap binaries and restart; state
# (~/.weaver: sqlite db, repos, worktrees, agent logins) survives.
#
# Usage:
#   ./deploy-loom-dgx.sh                 # full build + install + configure
#   ./deploy-loom-dgx.sh --restart-only # just restart the remote service
#   DGX_GITHUB_LOGIN=... ./...           # override seeded owner login
set -euo pipefail

DGX="${DGX:-100.121.8.110}"
DGX_USER="${DGX_USER:-$USER}"
DGX_GITHUB_LOGIN="${DGX_GITHUB_LOGIN:-dlwh}"
REMOTE="${DGX_USER}@${DGX}"
LOOM_SRC="${LOOM_SRC:-$HOME/src/loom}"
LOOM_ROOT="/opt/loom"                       # binaries; versioned releases + 'current' symlink
SERVICE_NAME="loom.service"
# Bind on the Tailscale IP only (not 0.0.0.0): this box also sits on cluster
# networks. The Mac reaches loom over Tailscale at this address.
BIND_ADDR="${BIND_ADDR:-${DGX}}"

log() { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

restart_only() {
  log "restarting ${SERVICE_NAME} on ${REMOTE}"
  ssh "$REMOTE" "sudo systemctl restart ${SERVICE_NAME} && sleep 2 && systemctl --no-pager --lines 5 status ${SERVICE_NAME}"
  exit 0
}
[ "${1:-}" = "--restart-only" ] && restart_only

# ---------------------------------------------------------------------------
# 0. Preflight
# ---------------------------------------------------------------------------
command -v ssh >/dev/null || die "ssh not found"
[ -d "$LOOM_SRC" ] || die "loom checkout not found at $LOOM_SRC (set LOOM_SRC=)"
command -v cargo >/dev/null || die "cargo not found"
rustup target list --installed 2>/dev/null | grep -q aarch64-unknown-linux-gnu \
  || die "missing cross target. Run: rustup target add aarch64-unknown-linux-gnu"
# The loom build needs a C linker for aarch64; on macOS Xcode's clang handles it.
# (If the link step fails with a missing linker, install llvm via brew and
# export CC_aarch64_unknown_linux_gnu / AR_* accordingly.)

ssh -o ConnectTimeout=5 "$REMOTE" true 2>/dev/null || die "cannot ssh to ${REMOTE}"

REV="$(git -C "$LOOM_SRC" rev-parse --short HEAD)"
log "target ${REMOTE} (aarch64 linux) · loom rev ${REV} · bind ${BIND_ADDR}:7878"

# ---------------------------------------------------------------------------
# 1. Cross-compile loom + tapestry (aarch64-unknown-linux-gnu)
#
# loom's build.rs builds the Vue SPA (npm/rspack) when frontend sources change;
# that output is static files, then bundled below via WEAVER_STATIC_DIR.
# ---------------------------------------------------------------------------
log "cross-compiling loom + tapestry (release, aarch64-unknown-linux-gnu)…"
(
  cd "$LOOM_SRC"
  cargo build --release -p loom -p tapestry --target aarch64-unknown-linux-gnu
)
for b in loom tapestry; do
  [ -f "$LOOM_SRC/target/aarch64-unknown-linux-gnu/release/$b" ] \
    || die "build did not produce $b"
done

# ---------------------------------------------------------------------------
# 2. Install binaries + SPA dist on the DGX
# ---------------------------------------------------------------------------
log "installing into ${LOOM_ROOT}/releases/${REV}/ on ${REMOTE}"
ssh "$REMOTE" "set -e
  sudo mkdir -p ${LOOM_ROOT}/releases/${REV}
  sudo chown -R \$(id -un):\$(id -gn) ${LOOM_ROOT}
"
scp -q \
  "$LOOM_SRC/target/aarch64-unknown-linux-gnu/release/loom" \
  "$LOOM_SRC/target/aarch64-unknown-linux-gnu/release/tapestry" \
  "$REMOTE:${LOOM_ROOT}/releases/${REV}/"
ssh "$REMOTE" "mkdir -p ${LOOM_ROOT}/releases/${REV}/dist"
scp -q -r \
  "$LOOM_SRC/crates/loom/static/dist/." \
  "$REMOTE:${LOOM_ROOT}/releases/${REV}/dist/"

# ---------------------------------------------------------------------------
# 3. Runtime prerequisites on the DGX (idempotent)
#
# loom shells out to: git, gh, node/npm (agents), jq. claude/codex CLIs are
# installed per-session by loom on first launch (no action needed). gh is
# already installed and logged in as ${DGX_GITHUB_LOGIN} on this box.
# ---------------------------------------------------------------------------
log "checking runtime prerequisites on ${REMOTE}"
ssh "$REMOTE" "set -e
  need=''
  for p in git gh node npm jq; do command -v \$p >/dev/null || need=\"\$need \$p\"; done
  if [ -n \"\$need\" ]; then
    echo \"installing:\$need\"
    sudo apt-get update -qq
    sudo apt-get install -y -qq \$need
  fi
  git lfs version >/dev/null 2>&1 || sudo apt-get install -y -qq git-lfs
  # Unprivileged user namespaces: needed by claude/codex sandboxing (bwrap).
  if [ \"\$(cat /proc/sys/kernel/unprivileged_userns_clone 2>/dev/null || echo 1)\" = 0 ]; then
    echo kernel.unprivileged_userns_clone=1 | sudo tee /etc/sysctl.d/99-loom-userns.conf >/dev/null
    sudo sysctl -p /etc/sysctl.d/99-loom-userns.conf
  fi
  true
"

# ---------------------------------------------------------------------------
# 4. Git credential plumbing (global config, idempotent)
#
# loom clones managed repos over plain https://github.com/... (its git helper
# passes a token only when a GitHub App is configured). The insteadOf rewrite
# routes those through SSH, authenticating with the box's existing SSH key,
# and gh's credential helper covers any remaining https auth.
# ---------------------------------------------------------------------------
log "configuring git/gh plumbing on ${REMOTE}"
ssh "$REMOTE" "set -e
  mkdir -p ~/.weaver
  git config --global credential.helper '!gh auth git-credential'
  git config --global url.'ssh://git@github.com/'.insteadOf 'https://github.com/'
"

# ---------------------------------------------------------------------------
# 5. systemd unit
#
# Notes:
#   - LOOM_OWNER_GITHUB seeds the bootstrap operator user on a fresh DB (re-runs
#     every boot, so setting it later + restart works too).
#   - LOOM_RUNNER unset = 'local' (ProcessRunner): session supervisors and
#     worktrees live on this host as plain processes/dirs.
#   - Bind the Tailscale IP. If tailscaled is slow at boot, Restart=always
#     retries until it can bind.
# ---------------------------------------------------------------------------
log "installing systemd unit ${SERVICE_NAME}"
ssh "$REMOTE" "set -e
  sudo tee /etc/systemd/system/${SERVICE_NAME} >/dev/null <<UNIT
[Unit]
Description=Loom (personal control plane for Arachne)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${DGX_USER}
Environment=WEAVER_HOME=/home/${DGX_USER}/.weaver
Environment=WEAVER_STATIC_DIR=${LOOM_ROOT}/current/dist
Environment=LOOM_OWNER_GITHUB=${DGX_GITHUB_LOGIN}
Restart=always
RestartSec=5
ExecStart=${LOOM_ROOT}/current/loom server run --addr ${BIND_ADDR}:7878

[Install]
WantedBy=multi-user.target
UNIT
  sudo systemctl daemon-reload
  sudo systemctl enable ${SERVICE_NAME}
  sudo ln -sfn ${LOOM_ROOT}/releases/${REV} ${LOOM_ROOT}/current
"

# ---------------------------------------------------------------------------
# 6. Start and wait for health
#
# Readiness probes are public (no auth): /api/health, /api/ready. We probe over
# loopback from the box itself.
# ---------------------------------------------------------------------------
log "starting ${SERVICE_NAME}"
ssh "$REMOTE" "set -e
  sudo systemctl restart ${SERVICE_NAME}
  for i in \$(seq 1 30); do
    if curl -fsS -m 2 http://127.0.0.1:7878/api/health >/dev/null 2>&1; then
      echo 'loom is healthy'
      exit 0
    fi
    sleep 1
  done
  echo 'loom did not become healthy; recent logs:'
  sudo journalctl -u ${SERVICE_NAME} -n 40 --no-pager || true
  exit 1
"

# ---------------------------------------------------------------------------
# 7. Post-start configuration (idempotent)
#
# The on-box loom CLI authenticates with the machine-local token (auto-minted
# on first boot, owner = seeded operator). Everything here is loopback.
#   - seed the Account PAT from the box's gh login, if none set;
#   - register managed repos (clone allowlist; clones happen lazily on first
#     session launch);
#   - mint a personal API token for the Mac, kept at ~/.loom-dgx-mac-token
#     (0600) so re-runs of this script re-print the same token.
# ---------------------------------------------------------------------------
log "configuring loom on ${REMOTE}"
ssh "$REMOTE" "set -e
  export PATH=\"${LOOM_ROOT}/current:\$PATH\"
  # Account PAT (used by interactive sessions for git/gh as ${DGX_GITHUB_LOGIN})
  if ! loom auth github-token get | grep -q '\"set\": *true'; then
    gh auth token | loom auth github-token set - || echo 'WARN: could not seed Account PAT (sessions will lack git push until set)'
  fi
  # Managed repos (clone allowlist) — add more here as needed
  loom repos register dlwh/arachne 2>/dev/null || true
  loom repos register marin-community/marin 2>/dev/null || true
  loom repos list
  # Personal API token for the Mac
  TOKEN_FILE=\"\$HOME/.loom-dgx-mac-token\"
  if [ ! -s \"\$TOKEN_FILE\" ]; then
    loom token add arachne-mac > /tmp/loom-token.out
    token=\$(head -n1 /tmp/loom-token.out)
    rm -f /tmp/loom-token.out
    [ -n \"\$token\" ] || { echo 'failed to mint token'; exit 1; }
    umask 077 && printf '%s' \"\$token\" > \"\$TOKEN_FILE\"
  fi
  echo
  echo '================================================================'
  echo ' Mac token (also saved on the DGX at ~/.loom-dgx-mac-token):'
  echo
  printf '   %s\n' \"\$(cat \"\$TOKEN_FILE\")\"
  echo
  echo ' From the Mac:'
  echo '   export WEAVER_API=http://${DGX}:7878'
  echo '   export LOOM_TOKEN=<token above>'
  echo '   # or: loom login dgx --url http://${DGX}:7878   (paste token)'
  echo '================================================================'
"

log "done — loom ${REV} running on ${REMOTE} at http://${DGX}:7878"
log "dashboard from the Mac (over Tailscale): http://${DGX}:7878"
