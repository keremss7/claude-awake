#!/usr/bin/env bash
# Wires four Claude Code hooks to Claude Awake. Strongly recommended.
#
# Without them the app can only ask "is the claude process running", which cannot
# tell a working agent from a session parked at an empty prompt — so it stays
# awake for both. With them, UserPromptSubmit/Stop bracket the real work and
# protection lifts as soon as the agent finishes.
#
# The overhead is one loopback curl per event.
#
#   bash install-hooks.sh            # add
#   bash install-hooks.sh --remove   # take them back out

set -euo pipefail

SETTINGS="${CLAUDE_SETTINGS:-$HOME/.claude/settings.json}"
MODE="${1:-add}"

python3 - "$SETTINGS" "$MODE" <<'PY'
import json, os, shutil, sys, time

path, mode = sys.argv[1], sys.argv[2]
MARKER = ".claude-awake/token"   # how we recognise our own entries
COMMAND = (
    'curl -sS -m 2 -X POST '
    '-H "X-Claude-Awake: $(cat ~/.claude-awake/token 2>/dev/null)" '
    '--data-binary @- '
    '"http://127.0.0.1:$(cat ~/.claude-awake/port 2>/dev/null)/hook" '
    '>/dev/null 2>&1 || true'
)

settings = {}
if os.path.exists(path):
    with open(path) as f:
        text = f.read().strip()
    settings = json.loads(text) if text else {}
    # Never edit a config file in place without leaving a way back.
    backup = f"{path}.bak-{time.strftime('%Y%m%d-%H%M%S')}"
    shutil.copy2(path, backup)
    print(f"backup: {backup}")

hooks = settings.setdefault("hooks", {})

def strip(event):
    """Drops our previous entries so re-running is idempotent."""
    groups = hooks.get(event, [])
    kept = []
    for group in groups:
        inner = [h for h in group.get("hooks", []) if MARKER not in h.get("command", "")]
        if inner:
            kept.append({**group, "hooks": inner})
        elif not group.get("hooks"):
            kept.append(group)
    if kept:
        hooks[event] = kept
    else:
        hooks.pop(event, None)

# SessionStart/SessionEnd bound the session and UserPromptSubmit/Stop bound each
# turn. PreToolUse/PostToolUse/Notification are heartbeats: a turn that runs for
# hours emits no Stop, so without them a long job ages out of the activity window
# and protection drops in the middle of exactly the work this tool exists for.
for event in (
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Stop",
    "PreToolUse",
    "PostToolUse",
    "Notification",
):
    strip(event)
    if mode != "--remove":
        hooks.setdefault(event, []).append(
            {"hooks": [{"type": "command", "command": COMMAND}]}
        )

if not hooks:
    settings.pop("hooks", None)

os.makedirs(os.path.dirname(path), exist_ok=True)
with open(path, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")

print("removed" if mode == "--remove" else "installed", "->", path)
PY
