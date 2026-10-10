# Selfhost macOS build

`scripts/build-selfhost.sh` builds the official `codeg` identity (`app.codeg`,
`codeg://`, port 3080 and existing data/keyring locations). It never launches
or installs the app. Do not launch it alongside another official-identity app.

## Choose signing explicitly

For a local Apple Development certificate, obtain its SHA-1 with
`security find-identity -v -p codesigning`, then set your own values:

```bash
export CODEG_SELFHOST_SIGNING_IDENTITY='<40-hex-certificate-SHA-1>'
export CODEG_SELFHOST_TEAM_ID='<10-character-team-id>'
scripts/build-selfhost.sh --bundles=app,dmg
```

Both variables are required for certificate-backed builds; missing or invalid
values fail before compilation. Existing selfhost users must now set these
variables locally instead of relying on machine-specific defaults. Do not
commit credentials. `CODEG_SELFHOST_SIGNING_KIND=developer-id` selects Developer
ID validation when using a Developer ID certificate; the default is `development`.

Without a certificate, use explicit ad-hoc signing:

```bash
CODEG_SELFHOST_SIGNING_IDENTITY=- scripts/build-selfhost.sh --bundles=app,dmg
```

Ad-hoc builds set `CODEG_COMPUTER_SIGNING=disabled`: **macOS computer use is
unavailable in unsigned builds**. The application refuses helper lookup/copy/
launch, and the helper refuses direct invocation and permission requests.
Removing Apple team checks would let another local process impersonate a peer;
we do not substitute a permissive requirement. Signed builds keep certificate,
team, launch-category and peer checks; cua-driver's separate trust pins are intact.

Neither local mode notarizes. Public CI uses the same ad-hoc policy when Apple
secrets are absent; fully configured Developer ID CI retains notarization.

## Output and verification

The default bundle is `app`; `--bundles=app,dmg` also produces a disk image.
Outputs stay in `src-tauri/target/release/bundle/`. `--stage` optionally copies
to `~/Applications/codeg-staged/codeg.app`, refusing existing destinations or
redirected stage directories. It never writes to `/Applications`.

The script disables updates with `CODEG_UPDATER=off`,
`NEXT_PUBLIC_CODEG_UPDATER=off` and `tauri.selfhost.conf.json` (no endpoints or
updater artifacts). `CODEG_FLAVOR` stays unset. Settings shows the Git commit;
the signed Info.plist contains `CodegBuildCommit`. The app version remains 0.34.0.

Sidecars are built first and the nested helper is signed before the outer app.
Certificate-backed builds use hardened runtime and pinned requirements; all
modes verify every nested signature, official identity, build stamp, absence of
entitlements and quarantine. The script uses the repo-pinned pnpm with `CI=true`,
four Cargo jobs, temporary data directories and this checkout's target directory.

Static inspection (does not run the app):

```bash
codesign --verify --deep --strict src-tauri/target/release/bundle/macos/codeg.app
codesign -dvvv src-tauri/target/release/bundle/macos/codeg.app
```

For an intentional installation, backup, handover and rollback, see
[SELFHOST.md](fork/SELFHOST.md). For an isolated side-by-side test app use
[build-fork.sh](../scripts/build-fork.sh) instead.
