# Selfhost macOS build

`scripts/build-selfhost.sh --stage` builds the official `codeg` identity
(`app.codeg`, `codeg://`, port 3080, existing data/keyring locations), signed
with Apple Development certificate SHA-1
`YOUR_SIGNING_IDENTITY_SHA1` (team `YOUR_TEAM_ID`).

The script only copies to `~/Applications/codeg-staged/codeg.app`. It refuses
an existing destination or redirected stage directories. It never launches
the app or writes to `/Applications`. Do not launch the staged app while the
official app is running: they intentionally share all data and ports.

The build sets `CODEG_UPDATER=off` and `NEXT_PUBLIC_CODEG_UPDATER=off`, leaves
`CODEG_FLAVOR` unset, and uses `tauri.selfhost.conf.json` to remove updater
endpoints/artifacts. Normal official builds retain their updater defaults;
Fork builds remain isolated. Settings displays `0.34.0 (fork <commit>)`, using
`CODEG_BUILD_COMMIT` from the current Git HEAD. The bundle version stays
`0.34.0`; commit stamping does not change update version comparisons.

Signing follows the upstream release workflow: build sidecars, sign the
nested helper first, then let Tauri sign the MCP sidecar and outer app. All
use hardened runtime without entitlements. Both helper trust requirements pin
the same certificate and team. `CODEG_COMPUTER_SIGNING=development` selects
Apple's Development launch validation category (3) for the helper; the
separately distributed cua-driver retains its Developer ID category (6),
vendor team, and pinned hashes. This local build is not notarized.

The script uses the repository's pnpm pin with `CI=true`, four Cargo jobs,
temporary build data directories, and this worktree's Cargo target directory.
Static gates check bundle identity, nested signatures, runtime flags, absence
of entitlements and quarantine before staging. Review the staged app with:

```sh
codesign --verify --deep --strict ~/Applications/codeg-staged/codeg.app
codesign -dvvv ~/Applications/codeg-staged/codeg.app
```

Installation and rollback belong to the later swap task. Preserve the existing
app and all data there; this script performs no migration or data operation.
