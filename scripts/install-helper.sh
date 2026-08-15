#!/usr/bin/env bash
# Installs the privileged half of Claude Awake as a LaunchDaemon.
#
# This is the one-time administrator step. Afterwards the app toggles protection
# over a local socket and never asks for a password again.
#
#   sudo bash install-helper.sh [/path/to/claude-awake-helperd]

set -euo pipefail

LABEL="com.keremseven.claude-awake.helper"
PLIST="/Library/LaunchDaemons/${LABEL}.plist"
TARGET="/usr/local/libexec/claude-awake-helperd"
SOCKET="/var/run/claude-awake.sock"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() { printf '\033[31m✗ %s\033[0m\n' "$1" >&2; exit 1; }
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; }

[[ "$(uname)" == "Darwin" ]] || die "This script is for macOS. On Windows use install-helper.ps1."
[[ "$(id -u)" -eq 0 ]] || die "Administrator rights required:  sudo bash $0"

# Inside an .app the helper sits in Contents/MacOS, one level up from this script.
find_helper() {
  local candidates=(
    "${1:-}"
    "${HELPER_BIN:-}"
    "${SCRIPT_DIR}/../MacOS/claude-awake-helperd"
    "${SCRIPT_DIR}/claude-awake-helperd"
    "${SCRIPT_DIR}/../src-tauri/target/release/claude-awake-helperd"
    "${SCRIPT_DIR}/../src-tauri/target/debug/claude-awake-helperd"
  )
  for c in "${candidates[@]}"; do
    [[ -n "$c" && -f "$c" ]] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

HELPER="$(find_helper "${1:-}")" || die "claude-awake-helperd not found. Build it first: npm run helper:build"
ok "helper found: ${HELPER}"

# A daemon must not run a binary from a directory a non-admin can write to.
install -d -m 755 -o root -g wheel /usr/local/libexec
install -m 755 -o root -g wheel "$HELPER" "$TARGET"
ok "installed: ${TARGET}"

cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${TARGET}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardErrorPath</key>
    <string>/var/log/claude-awake-helper.log</string>
</dict>
</plist>
PLIST_EOF
chown root:wheel "$PLIST"
chmod 644 "$PLIST"
ok "LaunchDaemon written"

# bootout of a service that is not loaded exits non-zero; that is expected.
launchctl bootout "system/${LABEL}" 2>/dev/null || true
rm -f "$SOCKET"
launchctl bootstrap system "$PLIST"

for _ in $(seq 1 25); do
  [[ -S "$SOCKET" ]] && break
  sleep 0.2
done
[[ -S "$SOCKET" ]] || die "Service started but the socket never appeared. Log: /var/log/claude-awake-helper.log"

ok "service running, socket ready: ${SOCKET}"
printf '\n\033[1mClaude Awake can now keep this machine running with the lid closed.\033[0m\n'
printf 'To remove it: sudo bash %s/uninstall-helper.sh\n' "$SCRIPT_DIR"
