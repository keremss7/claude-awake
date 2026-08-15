# Security Policy

## Reporting a vulnerability

Please report security issues privately through
[GitHub Security Advisories](https://github.com/keremss7/claude-awake/security/advisories/new)
rather than a public issue. Expect a first response within a week.

## What is in scope

This project ships a component that runs as root (macOS LaunchDaemon) or LocalSystem (Windows
service). The interesting surface is small and deliberately so:

| Surface | Notes |
|---|---|
| `/var/run/claude-awake.sock` | mode `0660`, owned `root:staff` — reachable by interactive macOS accounts |
| `\\.\pipe\claude-awake` | explicit DACL: SYSTEM and Administrators full, interactive users read/write |
| `127.0.0.1:47821` (hook listener) | runs unprivileged, requires the token in `~/.claude-awake/token` (`0600`) |

Design constraints the helper is meant to hold to:

- The `Request` enum is the entire vocabulary. Nothing derived from a request is ever executed.
- Every value written to the system is a compile-time constant.
- Only settings present in the baseline snapshot are modified, so every change is reversible.
- The first named-pipe instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so the name
  cannot be squatted by a process that starts earlier.

A report showing any of those broken is very much in scope.

## Known and accepted

- **Any interactive local user can toggle protection.** The socket and pipe are reachable by
  logged-in accounts by design — this is a personal-machine utility, and the worst a caller can
  do is keep the machine awake. It cannot read data or run commands.
- **Builds are unsigned.** Releases are not notarised (macOS) or Authenticode-signed (Windows).
  Verify what you run, or build from source.
