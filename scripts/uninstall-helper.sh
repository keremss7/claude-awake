#!/usr/bin/env bash
# Removes the privileged helper and — importantly — puts the power settings back
# before it goes, so uninstalling can never leave a laptop unable to sleep.
#
#   sudo bash uninstall-helper.sh

set -euo pipefail

LABEL="com.keremseven.claude-awake.helper"
PLIST="/Library/LaunchDaemons/${LABEL}.plist"
TARGET="/usr/local/libexec/claude-awake-helperd"
SOCKET="/var/run/claude-awake.sock"
STATE_DIR="/Library/Application Support/ClaudeAwake"

die()  { printf '\033[31m✗ %s\033[0m\n' "$1" >&2; exit 1; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$1"; }
warn() { printf '\033[33m!\033[0m %s\n' "$1"; }

[[ "$(id -u)" -eq 0 ]] || die "Administrator rights required:  sudo bash $0"

# Ask the still-running helper to roll back while it can still do it cleanly.
if [[ -S "$SOCKET" ]]; then
  printf '{"cmd":"restore"}\n' | nc -U "$SOCKET" >/dev/null 2>&1 || true
  ok "power settings restored"
elif [[ -f "${STATE_DIR}/baseline.json" && -x "$TARGET" ]]; then
  # Socket is gone but a snapshot survives: the helper restores it on startup.
  "$TARGET" &
  helper_pid=$!
  sleep 1
  kill "$helper_pid" 2>/dev/null || true
  ok "saved power settings restored"
fi

launchctl bootout "system/${LABEL}" 2>/dev/null || true
rm -f "$PLIST" "$TARGET" "$SOCKET"
rm -rf "$STATE_DIR"
ok "service and files removed"

# Belt and braces: if anything above failed, this is the setting that actually
# matters, and leaving it on would be the one genuinely harmful leftover.
if [[ "$(pmset -g 2>/dev/null | awk '/SleepDisabled/ {print $2}')" == "1" ]]; then
  pmset -a disablesleep 0 && warn "disablesleep was still on; turned it off"
fi

printf '\n\033[1mRemoved.\033[0m Drag Claude Awake.app to the Trash to finish.\n'
