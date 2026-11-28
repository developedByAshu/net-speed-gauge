# Mac test tools

Helpers for testing a release on a real Mac: updating through **Check now**, then clicking through the
menu bar icon and Settings. The same tools sit in Mini Bar, Juice Bar and NetSpeedGauge.

| Tool | What it does |
|---|---|
| `update_check.sh` | Backs up the settings before a test, then checks the version, the relaunch, the signature, leftovers and the settings after it |
| `input.swift` | Real clicks, right clicks, pointer moves and Esc on the screen |
| `windows.swift` | Lists a process's windows, once or every half second, to see whether its menu is open |

Build the Swift tools once: `swiftc -O input.swift -o input` and `swiftc -O windows.swift -o windows`.

## An update test

1. `./update_check.sh backup JuiceBar /tmp/jb-backup`
2. Take a screenshot of the menu bar to compare the icon order afterwards.
3. Open Settings > General, click **Check now**, wait for "Updates to X when Settings closes", close Settings.
4. `./update_check.sh verify JuiceBar 0.1.2 /tmp/jb-backup`
5. Open every Settings page and both clicks on the icon, then take the menu bar screenshot again.

## Gotchas

- Show a "hands off" note before sending input, and put the pointer back afterwards (`input pos` first).
- Never take a screenshot between opening a menu and the `esc` that should close it. The capture takes
  the key focus, the Esc never arrives, and the menu stays open (it looks like an app not closing).
- A menu that is still open shows as a layer 101 window in `windows PID`.
- Run these from Terminal, never from inside a test app bundle, so no app asks for screen permission.
