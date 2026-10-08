# Codeg-Fork desktop flavor

## Contract and design

`CODEG_FLAVOR=fork` is a compile-time Rust choice; unset or any other value is
Official. `NEXT_PUBLIC_CODEG_FLAVOR=fork` selects the static frontend. The fork
build wrapper sets both, builds both sidecars with the same environment and
applies `tauri.fork.conf.json`. Official identifiers and default values remain
unchanged. Message content links (`codeg://agent/…`, `codeg://session/…`) remain
unchanged.

The initial audit was completed before implementation against the C1 baseline;
the inherited-environment row was added after the failed first smoke exposed
that omission.
“Explicit” means a user/API/agent action rather than startup. Paths in the table
are relative to `src-tauri/` unless stated otherwise.

## Collision audit

| Location | Shared identity/location | Trigger | Fork handling |
| --- | --- | --- | --- |
| `tauri.conf.json` | `codeg`, `app.codeg`; app data, WebKit storage, single-instance identity | Startup | Overlay `Codeg-Fork`, `app.codeg.fork` |
| `tauri.conf.json` deep-link | OS scheme `codeg` | OS registration/startup | Overlay `codeg-fork`; retain message content links |
| `tauri.conf.json`, `lib.rs`, app-update commands/handlers, frontend updater | Official release endpoint, updater plugin, signed updater artifacts | UI automatic checks / explicit install | Empty endpoint array, artifacts false; no fork updater plugin/check/install |
| `tauri.conf.json` externalBin, `tauri.macos.conf.json` | `codeg-mcp`, nested `codeg-computer-helper.app` | Build/launch | Bundle-local filenames retained; all compiled with fork flavor |
| `windows/installer-hooks.nsh`, `linux/main.desktop` | Installer-scoped sidecars, desktop template | Install | Existing paths scoped to install; fork wrapper only builds macOS app/optional dmg |
| `paths.rs` | `.codeg`; pets, browser-profiles, uploads, backgrounds, logs, turn-timings, acp-transcripts | Automatic / explicit | `.codeg-fork`; existing CODEG_HOME/CODEG_DATA_DIR override order preserved |
| `main.rs`, all binary entry points, `paths.rs` | Inherited CODEG_HOME/DATA_DIR/STATIC_DIR/ACP_TMP_ROOT may target official state | Process launch from official app | Before any logging/runtime, discard overrides under official home/data/cache roots; retain ordinary custom roots |
| `preferences.rs` | `~/.codeg/preferences.json` | Early startup read / explicit write | Fork home; honor explicit test home without changing official default |
| `process.rs` | `~/.codeg/npm-global` | Startup PATH setup / CLI install | Fork home npm prefix |
| `commands/experts.rs`, science/custom-skills/office-tools | `~/.codeg/skills`, agent skill directories | Startup central installation / explicit symlink | Fork central store; reject fork writes to shared agent directories |
| `commands/backup/{sections,restore,source}.rs` | Managed roots, central skills; `.codeg-restore-*` within data dir | Explicit backup/restore; startup pending restore | Flavor-aware roots; relative archive names retained for compatibility |
| `workspace_state/mod.rs` | `.codeg-edit-*.tmp` inside selected workspace | Explicit edits | Workspace-local temporary files, no app-global state |
| `keyring_store.rs` | keychain service `codeg`; server `codeg`/`.codeg-data` fallback | Explicit credential operations | `codeg-fork`, server `codeg-fork`/`.codeg-fork-data` |
| `commands/clipboard.rs` | OS clipboard; no keyring Entry present in this baseline | Explicit copy/paste | Shared OS resource by user action, no persisted app identity |
| `git_credential.rs` | fallback data path `app.codeg` (three platforms) | Credential helper subprocess | Flavor app identifier; generated scripts already carry effective data dir |
| `acp/binary_cache.rs` | current and legacy `app.codeg/acp-binaries` | Startup migration/sweep, on-demand install | Flavor current and legacy roots; never migrate official cache into fork |
| `acp/connection.rs` | `app.codeg/acp-logs` | Agent launch | Flavor app identifier |
| `computer/helper/driver_proc.rs` | `app.codeg/computer-helper` | Helper launch | Flavor app identifier |
| `computer/local.rs`, helper plist, prepare-sidecars | `app.codeg.computer-helper`, TCC/signing identity | Helper launch / build | `app.codeg.fork.computer-helper`; fork helper display name |
| `computer/agent.rs`, targets, companion | Own app `app.codeg`, `/Applications/codeg.app`; test fixtures | Computer-use safety policy | Recognize fork identity while preserving protection for official app; fixed fixtures reviewed |
| `lib.rs` single-instance | Tauri identifier-derived process lock | Startup | Overlay identity separates instances |
| `lib.rs`, system-settings autostart | product-name-derived LaunchAgent/login registration | Explicit enable/disable | Fork disables shared startup registration; no LaunchAgent/login-item writes |
| `commands/notification.rs` | `app.config().identifier` | Notification | Overlay provides fork macOS notification identity |
| `web/mod.rs`, `bin/codeg_server.rs` | default port 3080 | Service start | 3090; development wrapper 127.0.0.1:3091 |
| `bin/codeg_server.rs` | `data_dir/codeg`, `.codeg-data` | Server startup | Fork defaults; wrapper uses isolated temp home/data |
| `commands/mcp.rs`, web MCP handlers | global Claude/Codex/Gemini/OpenCode/etc config files | Explicit config update/sync | Fork rejects writes to global agent MCP configuration |
| `acp/connection.rs` Antigravity settings sync | writes agent settings.json before launch | Automatic on agent launch | Fork suppresses global settings write |
| `acp/connection.rs` Hermes reconciliation | updates ~/.hermes/.env before launch | Automatic on agent launch | Fork suppresses reconciliation |
| `commands/acp.rs` native CLI settings/login/install APIs | CLI-owned config, credentials, system npm prefix | Explicit settings/install/login only | Remain explicit integrations; no such action in smoke. Fork npm fallback is private |
| `commands/config_sync` | WebDAV credentials and auto upload | Auto only after explicit configuration; import explicit | Isolated DB/keyring; no shared startup configuration |
| Git credentials / terminal env | `GIT_CONFIG_COUNT/KEY/VALUE`, askpass scripts | Child launch | Already process-local and data-dir scoped; no ~/.gitconfig write found |
| `acp/delegation/listener.rs` | `codeg-delegation-{pid}.sock`, uid short socket dir | Startup | PID scoped; separate running instances cannot collide |
| `acp/scratch_dir.rs`, other temp files | `codeg-acp` temp subtree and owner records | Startup sweep / agent launch | Namespace fork subtree; keep PID/owner protections |
| Frontend layout/tasks/stash/settings | product labels, 3080 defaults | Render | Flavor constants, preserving official casing at call sites |

## Verification plan

1. Pure contract tests assert all Official literals and Fork values, unknown
   flavor fallback, guards and helper identity. Verify compile-time selection.
2. Route audited call sites, guard shared writes and retain content-link syntax.
3. Add safe build/server wrappers and test overlay/helper metadata consistency.
4. Run desktop/server/MCP check, test and clippy, frontend lint/test/build, then
   build only through `scripts/build-fork.sh`.
5. Inspect main/helper Info.plists; smoke for 60 seconds with lsof every 2 seconds;
   check official PID and exclusive port 3080 ownership before/after; stop only
   the verified fork PID. Store evidence in `.codex/` and the C1 report.

## Operational boundaries

No fork build is installed into `/Applications`. Optional `--install` is limited
to `~/Applications/Codeg-Fork.app`. Environment overrides remain supported, but
Fork startup discards any pointing under the known Official home/data/cache
roots before preferences, logs, credentials, or the runtime initialize. Global agent
configuration and autostart management are intentionally unavailable in Fork.
Agent transcripts / executables can still be discovered from their normal
locations; that is explicit integration with external tools, not app-owned state.

The global skills resolver is disabled in Fork for both reads and writes, so a
later unlink/delete path cannot bypass a guard placed only on link creation.
Native CLI configuration/login/install APIs remain explicit user operations;
this flavor is not a sandbox for external agent executables.

Compatibility note: the three legacy home-only consumers (early preferences,
central skills and npm fallback prefix) now honor an explicitly set CODEG_HOME.
Their Official defaults are byte-identical. This closes the test/custom-home
escape without changing installations that use the upstream default home.

No localizable UI copy is added: Codeg-Fork is a proper product name; existing
translations and message content remain unchanged. Backend guard diagnostics
follow the repository's English error-message convention.

## Usage

Build with `scripts/build-fork.sh`; optional `--dmg` adds a disk image and
`--install` copies only to `~/Applications/Codeg-Fork.app`. Installation refuses
an existing destination or redirected Applications directory. The wrapper uses
pnpm 10 locally without changing the upstream package-manager pin or user
configuration. Ad-hoc signing is used, with official helper trust anchors cleared.

For headless testing, `scripts/fork-server-dev.sh` creates fresh temporary home,
data and scratch directories, prints a generated token, and listens on
`127.0.0.1:3091`. Browser bridge listeners are disabled. Optional CODEG_PORT must
be in 3090–3099 or 18400–18499. On exit it stops only the verified child server
and retains the temporary state for inspection.

## Regression found during smoke

The first smoke inherited `CODEG_DATA_DIR` from the official app hosting the
agent. That override took precedence over the correct bundle identity and
opened official DB/log paths. Sampling stopped the fork after 2.56 seconds;
the official PID and exclusive port 3080 listener remained alive. No inspection
or rollback of official data was attempted. The first launch may have written
startup data or logs; isolation evidence must use the post-fix smoke, not this
failed attempt.

The fix runs at every binary's synchronous entry point. It compares normalized
paths without opening official directories and removes inherited overrides
that target their roots or descendants. Regression checks cover Official
preservation, Fork removal, custom/Fork root preservation, dot components and
case variants on macOS/Windows. The final smoke retains the original inherited
environment to exercise this protection.

## Verified result

- Desktop check/test/clippy: passed; 5,074 tests passed, 3 existing ignored.
- Server check/test/clippy: passed; 4,755 tests passed, 2 existing ignored.
- MCP check/clippy and binary test target: passed (binary contains no tests).
- Fork contract, path/guard integration, inherited-root and deep-link tests:
  passed. Frontend lint, full suite (8,395 tests), final focused suite and build:
  passed.
- Main/helper plist and ad-hoc signatures: valid; updater artifacts absent.
- Final smoke: 31 lsof samples over 60 seconds; 60.81 seconds total including
  verified Fork shutdown. Fork PID 40575 opened only its own app-state roots;
  no official data/home paths were present. Official PID 87598 remained alive
  and the only 3080 listener before and after. The launch intentionally retained
  the inherited official CODEG_DATA_DIR to verify the startup guard.
- Headless wrapper: authenticated localhost:3091, disabled update endpoint,
  isolated temporary state and verified shutdown all passed.

The bundle remains at
`src-tauri/target/release/bundle/macos/Codeg-Fork.app`; it was not installed.
Detailed commands, failures and evidence paths are in the program's C1 report.
