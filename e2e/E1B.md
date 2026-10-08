# E1b Simulator verification helpers

These test-only helpers use the E0 environment named `e1b` and the E1b iOS
worktree's `E1bUI` XCUITest scheme. They never modify the app under test.
The full scenario evidence and acceptance report live in
`/Users/you/Projects/codeg-fork-plan/reports/E1b.md`.

Start `ui_bridge.py --port 49994 --config <ios-worktree>/CodegiOSUITests/E1bController.json`
before building/running the UI scheme. The ignored config contains only the
loopback URL. `ui_command.py` sends accessibility actions and saves sanitized
trees/screenshots. Credentials are transferred only through `/secret` to the
runner's pasteboard, then pasted into secure fields; never put them in commands.
The driver clears its pasteboard when it exits. A `finish` command ends the test.

`ios_scenarios.py` provides fault control, UI captures, timing, and API helpers.
`e1b_cards.py` requires session 1 to be open and a live custom mock connection.
It verifies all three pending-card types before and after reconnect.

`e1b_lifecycle.py` stops/restarts only the E0-recorded home or relay processes,
checking PID identity and the official-app baseline. `move-home` uses an
ephemeral loopback listener so 3094 becomes unreachable while the in-process
relay service remains available. `restore-home` returns to 3094. `rekey-relay`
removes only the owned temporary relay TLS directory, restarts it, and updates
the isolated home pin. Run rekey last: an existing terminating proxy retains its
old upstream pin until explicitly restarted.

Proxy evidence includes timestamps, HTTP body byte counts, paging selectors,
attach cursors, frame types, and SHA-256 of `X-E1b-Proof`. It deliberately excludes
tokens, header values, and message/tool payloads. Header digest is evidence of
presence, not a substitute for secret handling.

These helpers have fixed E1b paths/ports and are not a general CI suite. Keep the
official app/3080, real devices, global configuration, and other agents' ports
untouched. Finish the driver, run `env-down.sh --name e1b`, stop the exact bridge
PID, and shut down only Simulator E1b after verification.
