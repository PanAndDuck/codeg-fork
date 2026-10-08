# codeg relay E2E harness

Python 3 standard library only; no LLM account, token, package install or real
agent invocation. Run from a codeg worktree containing this `e2e/` directory.
Requires the existing Rust toolchain, `openssl`, `ps`, `lsof`, and Bash.

## Safety and lifecycle

Start **every shell** with:

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
```

The two shell wrappers do this internally too. All launched processes receive
explicit disposable `CODEG_DATA_DIR` / `CODEG_HOME`; `TMPDIR`, `TMP`, `TEMP` are
also replaced. The harness checks `/Applications/codeg.app` PID 87598 and its
exclusive ownership of 3080 before/after lifecycle operations, without making
requests to 3080 or opening official data. It refuses occupied ports and URLs
outside loopback/test ports. No global configuration, HK access or device install.

`env-up` builds the release relay in this worktree and uses C1's
`scripts/fork-server-dev.sh` to build/start the fork server. C1 fixes
`CODEG_FLAVOR=fork`, browser bridge off, fresh home/data/scratch and random web
token. `--skip-build` skips only the relay build; the C1 wrapper still invokes
incremental Cargo for the server. `CARGO_BUILD_JOBS=4`, Rustup PATH and both local
target directories are explicit. The API harness does not build frontend assets;
an empty `out/` is created if necessary to satisfy the Cargo resource check.

State: `e2e/.state/NAME.json` (lifecycle), `NAME.env` (shell),
`NAME.session.json` (latest folder/conversation/connection), all private. State
contains **disposable secrets** and is ignored by Git. Lifecycle writes are
atomic; a per-name lock prevents simultaneous owners. Temporary data lives under
`/private/tmp/codeg-e2e-NAME-*` on macOS. Startup failure rolls back owned state.
Keep the startup shell until it reports `connected`; Ctrl-C/SIGTERM cancels it.

`env-down` checks each recorded PID's complete command and start time before
signalling it. It stops descendants from its verified process tree, removes only
its marked temporary root and credential state, and checks all four ports are
free. Evidence JSON files remain. No `pkill`, process-name cleanup or global
app shutdown is used.

Local Cargo build outputs, the resource-check `out/` directory and empty `.lock`
files remain for reuse. Lock files are deliberately not unlinked: deleting an
inode used by another waiter could allow two lifecycle owners. None is a running
process or a credential/data directory.

## E1a exact commands

In the assigned codeg worktree after the orchestrator integrates E0:

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
./e2e/env-up.sh --name e1a --relay-port 18453 --server-port 3093 --proxy-port 18454 --control-port 18445 --proxy-mode terminate
python3 e2e/register_mock.py --name e1a --connect
python3 e2e/smoke.py --name e1a
python3 e2e/smoke_faults.py --name e1a
python3 e2e/environment.py restart-proxy --name e1a --proxy-mode passthrough
python3 e2e/smoke_passthrough.py --name e1a
python3 e2e/environment.py restart-proxy --name e1a --proxy-mode terminate
python3 e2e/environment.py restart-relay --name e1a --downtime 10
./e2e/env-down.sh --name e1a
```

The smoke tools create their own conversations and disconnect their agents at
the end. To keep one open for manual work, run `register_mock.py --connect`
afterwards. No need to call an install/download API. `acp_save_custom_agent`
registers a local binary (`custom:e2e-mock`), `acp_update_agent_env` enables it,
and create/connect calls all go through pinned HTTPS by default. Its archive URL
is deliberately `example.invalid`; the executable resolves from the fork
server's process-local PATH. `supportsMcp=false` prevents companion injection.

## E1b exact commands / Simulator pairing

In the assigned codeg harness worktree (independent ports from E1a):

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
./e2e/env-up.sh --name e1b --relay-port 18463 --server-port 3094 --proxy-port 18464 --control-port 18447 --proxy-mode terminate
python3 e2e/register_mock.py --name e1b --connect
source e2e/.state/e1b.env
python3 - <<'PY'
import json, sys
sys.path.insert(0, 'e2e')
from harness import load_state
s = load_state('e1b')
for title, url, fp in [('relay', s['relay_url'], s['fingerprint']),
                       ('fault proxy', s['proxy_url'], s['proxy_fingerprint'])]:
    print(json.dumps({'v': 1, 'mode': 'relay', 'url': url, 'token': s['token'],
                      'fp': fp, 'name': 'E1b ' + title}))
PY
```

Use these **test-only** values in the fork Simulator app. The terminating proxy
has its **own** fingerprint. Pass-through uses the relay fingerprint. Switching
back to termination regenerates the proxy identity; reread state and re-pair.
For I6 use direct `http://127.0.0.1:3094` as primary and relay/proxy as secondary.
Simulator creation/build/UI driving remain E1b-owned; nothing here installs apps.

Cleanup:

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
./e2e/env-down.sh --name e1b
```

Defaults, if no port flags: relay 18443, server 3091, proxy 18444, control 18445.
Server ports must be 3090–3099, relay/proxy 18400–18499, control 18445–18449.
`--no-proxy` starts only relay + server. `restart-proxy` also starts a missing proxy.

## Mock language

One directive per line, in the order written. Plain text is echoed if no
directive lines exist. With directives, non-directive context is ignored (codeg
may prepend workspace context). Each prompt is a turn; no filesystem/tool
commands are actually executed, even when a tool card is displayed.

| Directive | Behavior |
| --- | --- |
| `#stream lines=50 interval_ms=50` | Emits `line 000001\n` … `line 000050\n`; defaults 20/50; numbering resets per directive. |
| `#tools count=250 output_kb=8` | Emits 250 `tool_call` + completed `tool_call_update` pairs; each text output exactly 8192 bytes. Optional `interval_ms=0`; IDs include turn and tool index. |
| `#sleep 30` | Cancellable pause in seconds (fractional allowed, max 3600). |
| `#echo fixed marker` | Verbatim text plus newline, no shell interpretation. |
| `#thought checking` | `agent_thought_chunk`. |
| `#plan steps=3` | Standard ACP `plan` with numbered pending entries. |
| `#permission` | Standard `session/request_permission`; awaits `allow_once` / `reject_once` or cancellation. No real action follows a denial; subsequent directives continue. |
| `#question` | codeg-supported `_x.ai/ask_user_question` with Alpha/Beta options; blocks for response. |
| `#approve_plan` | codeg-supported `_x.ai/exit_plan_mode` approval request; blocks for response. |
| `#fail` | Prompt JSON-RPC error `-32000`, message `scripted mock failure`. |

Unknown directives/invalid arguments fail explicitly. Budgets: stream ≤100,000
lines, tools ≤10,000, individual output ≤1 MiB, combined tool text ≤48 MiB.
Initialization advertises ACP v1, `loadSession=true`, empty auth methods. New
sessions use fresh IDs; load accepts an existing ID as a **stateless mock** and
does not replay history. Codeg persists its own transcript. Cancel is a
notification; prompt returns `stopReason=cancelled`; normal completion returns
`end_turn`. Stdout is newline-delimited JSON-RPC only. Reader and prompt worker
are independent, so cancellation and permission responses work during a sleep.

Large **live** snapshot recipe:

```text
#tools count=250 output_kb=8
#sleep 30
```

`turn_complete` clears live tools server-side, so fetch/attach during the sleep.
Smoke uses a 5-second pause and measured HTTP 2,156,857 bytes / WS 2,163,304 bytes.

### ACP completion ordering finding

The current server can emit `turn_complete` before it handles the last ACP
notification, even though the mock writes the notification first on stdout.
The mock defaults to an explicit **100 ms settle window** before the prompt
response to keep recovery tests reproducible. This is a test timing choice,
**not a production fix or guarantee**. Disable it to investigate:

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
python3 e2e/probe_completion_order.py --name e1a --settle-ms 0 --turns 5
python3 e2e/probe_completion_order.py --name e1a --settle-ms 100 --turns 5
```

The probe exits 1 on late/missing-at-completion content; writes sanitized JSON
evidence. E0's recorded repeat: 2/5 deviations at 0 ms, 0/5 at 100 ms (an earlier
run was 4/5 at 0 ms). E1a should report/follow up this product race. Registration
also accepts `--settle-ms`; each new mock process receives the chosen value.

## API and WS client

`client.py --api COMMAND --json JSON` sends pinned HTTPS through relay by default;
`--through proxy|server` selects another owned endpoint. Responses are printed
as JSON; `--bytes` prints raw response byte count. No official/global token input
is supported. Look up IDs in `e2e/.state/NAME.session.json`.

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
python3 e2e/client.py --name e1a --api get_relay_status
python3 e2e/client.py --name e1a --api acp_prompt --json '{"connectionId":"CONNECTION_ID","conversationId":1,"folderId":1,"clientMessageId":"manual-unique-1","blocks":[{"type":"text","text":"#stream lines=300 interval_ms=200"}]}'
python3 e2e/client.py --name e1a --api acp_get_session_snapshot --json '{"connectionId":"CONNECTION_ID"}' --bytes
python3 e2e/client.py --name e1a --through proxy --watch --connection CONNECTION_ID --since-seq 17 --seconds 60
```

Replace the IDs with printed values; use a unique `clientMessageId` per turn.
Responses/snapshot fields are **snake_case**. Command parameters are camelCase.
WS uses `codeg-events, codeg-token.<base64url>` auth, `action=attach` with
`subscription_id`, `connection_id`, optional `since_seq`. Snapshot/replay/event
are distinct; replay threshold is 32 events. Global `{channel,payload}` messages
also appear and should not be counted as a second copy of a subscribed event.

## Fault proxy control

The control listener is HTTP **127.0.0.1 only**, unauthenticated test tooling.
`GET /status` or `POST /status` returns mode, active connection counts, configured
faults and counters. `POST /control` accepts the following JSON. `faultctl.py`
constructs it without handling credentials:

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
python3 e2e/faultctl.py --name e1b block_ws --seconds 30
python3 e2e/faultctl.py --name e1b omit_since_seq --count 1
python3 e2e/faultctl.py --name e1b status
python3 e2e/faultctl.py --name e1b reset
# Equivalent control request; no codeg token needed:
curl --fail http://127.0.0.1:18447/control -H 'Content-Type: application/json' -d '{"command":"drop_ws"}'
```

| Command / optional fields | Semantics |
| --- | --- |
| `drop_ws` | Close currently upgraded sockets. In pass-through, closes **all current TCP connections**. |
| `block_ws`, `seconds=30` | Also drops existing WS; reject new upgrades until deadline; HTTP continues in terminate mode. Pass-through blocks **all** new TCP connections. |
| `drop_all` / `block_all`, `seconds=30` | Close all data connections / reject all new data connections for duration; control stays available. |
| `throttle`, `bps=20000` | Both directions, per connection; 0 disables. Bytes/sec, not bits/sec. |
| `blackhole_after_attach`, `count=-1` | Terminate only: ready passes; after forwarding client attach, drop **all** downstream WS frames including Ping/Pong/snapshot/replay. HTTP remains available. |
| `omit_since_seq`, `count=1` | Terminate only: remove cursor from next N attaches, forcing snapshots. |
| `inject_lagged` | Terminate only: inject detached for each current subscription; suppress that old subscription's updates until new attach/reset. |
| `drop_http_response`, `path=/api/acp_prompt`, `count=1`, `close_ws=false` | Terminate only: forward POST fully, read upstream response, discard response and close downstream. Optional `close_ws` drops WS too. Counters record upstream HTTP status and prompt request count. |
| `quiet_after_first_content`, `count=-1` | Terminate only: after first subscribed content/tool frame, drop subsequent downstream JSON (including complete/status); WS control frames still flow. Applies to existing sockets and configured future attaches. |
| `drop_attach`, `count=-1` | Terminate only: close upon receiving attach, before snapshot. Use count=2 for two initial failures. |
| `delay_attach`, `seconds=8`, `count=-1` | Terminate only: delay forwarding attaches. |
| `delay_http`, `path=/api/acp_connect`, `seconds=8`, `count=1` | Terminate only: delay a selected request before forwarding. |
| `reset` | Clear faults/throttle/block deadlines and live fault flags; keep counters for evidence. |

`count=-1` persists until reset; positive counts are consumed. Most faults are
armed for future attach/request; `drop_*`, `block_*`, `inject_lagged` act now.
`quiet_after_first_content` can arm an already streaming socket; its next content
frame passes and starts the quiet period. Reset cannot resurrect closed sockets
or recover intentionally discarded content: reattach via cursor/snapshot.

Pass-through never decrypts TLS; pinning sees the relay certificate unchanged.
Use a dedicated WS route/client if you need an opaque WS-only outage while HTTP
uses the relay directly. Termination uses a disposable openssl self-signed cert
and independently verifies the upstream relay fingerprint **before** forwarding
credentials. It negotiates HTTP/1.1, removes WS compression negotiation, handles
masked/fragmented frames and 64 MiB messages. Non-WS requests use a fresh upstream
connection with `Connection: close`; request limit 16 MiB, HTTP head 64 KiB.
It is a functional fault harness, not a keep-alive/HTTP2 performance benchmark.

## I2's 14 scenarios → trigger map

All commands below assume `python3 e2e/faultctl.py --name e1b COMMAND ...`.
Use **terminate** unless a row explicitly permits pass-through. UI observations
and acceptance criteria remain in I2's report; E0 only proves fault generation.

| I2 | Mock / action | Proxy command / mode |
| --- | --- | --- |
| 1 normal | `#stream lines=20 interval_ms=100`; `#tools count=1 output_kb=1` + `#sleep 20` + `#echo done` | Either mode; no faults. |
| 2 relay restart | `#stream lines=600 interval_ms=100` | `environment.py restart-relay --name e1b --downtime 10`; same key/cert, new owned PID. |
| 3 finish during gap | `#stream lines=30 interval_ms=100`; cut after first line | `block_ws --seconds 30`; arm `omit_since_seq --count 1` for idle snapshot; repeat without omit for replay. HTTP stays live. |
| 4 background | 60 s turn, Simulator Home 30 s, reopen; repeat short turn | No proxy required. Do not change the host network. |
| 5 path change substitute | Long turn | `drop_ws` in either mode; real cellular/NWPathMonitor reason is not generated by this proxy. |
| 6 lagged | Long turn after a few lines | `inject_lagged`; new attach must carry cursor. |
| 7 ambiguous prompt | Arm before Send | `drop_http_response --path /api/acp_prompt`; repeat `--close-ws`; verify `prompt_requests` rises by one. For 409, submit while `#sleep 30` is active. |
| 8 reattach pre-snapshot failure | Other client starts long turn, then open session | `drop_attach --count 2`; third attach passes. |
| 9 20 s deadline | Arm before Send/reattach | `blackhole_after_attach`; observe >20 s, then `reset`. New-send prompt count must not rise before attach succeeds. |
| 10 quiet watchdog | Arm then send short turn; repeat long `#sleep 60` | `quiet_after_first_content`; HTTP snapshot remains available, Ping/Pong still work. |
| 11 ≥6 attempts / Retry now | Keep long turn active | `block_ws --seconds 300`; after ≥6 attempts `reset`, tap Retry now. |
| 12 foreground during send | Background/foreground during initial send | `delay_http --path /api/acp_connect --seconds 8`; or `delay_attach --seconds 8 --count 1`. |
| 13 idle touch | Finish turn, stay 190 s, leave 40 s | No fault. Compare `status.counters.touch_requests` and `last_touch_unix`; caller already knows connection ID. |
| 14 pending cards / editor | `#permission`, `#question`, `#approve_plan`; resolve via another client during outage | `drop_ws` / `block_ws`; use APIs below; mention/keyboard/pinning regressions need Simulator UI checks. |

Pending card response API parameters (read request IDs from live snapshot):

```json
{"connectionId":"C","requestId":"pending_permission.request_id","optionId":"allow_once"}
{"connectionId":"C","questionId":"pending_question.question_id","answer":{"answers":[{"questionId":"pending_question.questions[0].id","labels":["Alpha"]}]}}
{"connectionId":"C","approvalId":"pending_plan_approval.approval_id","answer":{"decision":"approve","feedback":""}}
```

Commands respectively: `acp_respond_permission`, `acp_answer_question`,
`acp_answer_plan_approval`. `smoke_faults.py` demonstrates all three end to end.

## Verification and boundaries

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then exit 1; fi
python3 -m unittest discover -s e2e/tests -v
bash -n e2e/env-up.sh e2e/env-down.sh
python3 -m compileall -q e2e
git diff --check
```

The core smoke asserts contiguous event sequences, exactly 50 numbered lines,
completion, >1 MiB HTTP and WS snapshots, and small-gap replay after drop.
Fault smoke checks 12 groups, including 21 s downstream silence and three card
types. Pass-through smoke checks the unchanged relay pin and cursor recovery.
All clients disconnect on exit; lifecycle cleanup remains `env-down`.

Not E0 claims: no Simulator UI/network-change/touch-duration acceptance, no
desktop relay-settings acceptance, no HK/device deployment. Server auth limiter
exempts loopback addresses; this proxy does not spoof remote peer IPs and cannot
prove C3's non-loopback ban behavior. The completion-order race remains a product
follow-up. Raw server logs can contain test credentials/prompt payloads and stay
private in disposable state; do not commit them. Proxy logs contain only startup
mode/ports; control counters contain no token/header/prompt values.

## E1a additional verification

`e1a_scenarios.py --evidence DIRECTORY SCENARIO` runs `latency` (300 lines on
each route), `paired` (same deltas on direct and relay sockets), `large`,
`pressure`, `heartbeat`, `no_read`, or `restart`. It uses `--name e1a` by default.
Run these serially against the E1a environment after the baseline smoke scripts.
`pressure` sends app Ping and a second Attach on the same throttled socket. Its
JSON includes byte progress, downstream EOF and sampled relay stream counts.
Ignore initial stale zero counters until a positive active count is observed.
For deadline attribution, enable `RUST_LOG=info,codeg_lib::web::ws=debug` on the
owned server: 30-second write deadlines begin at a blocked write, not prompt
start, and proxy buffers can delay downstream EOF further.

`e1a_relay_checks.py --evidence DIRECTORY SCENARIO` runs `basics` (temporary
relay 18490, cap=2), `limiter` (existing en0 source IP to loopback destination),
`replacement` (temporary server 3096), or `offline` (stops the owned home server;
run last). No network-interface changes or off-host requests are made.
The unaffected alternate-IP check uses loopback, which C3 deliberately exempts;
it does not prove isolation between two non-loopback IP buckets.

`e1a_monitor.py --evidence DIRECTORY` samples explicit owned PIDs until
`DIRECTORY/monitor.stop` exists. Optional `desktop-owned.json` contains the
desktop process identity recorded after an absolute-path `open -n` launch.
`swift e2e/e1a_decode_qr.swift WINDOW.png PRIVATE.json` decodes one actual QR
with macOS Vision when zbarimg is unavailable. QR screenshots and decoded
payloads contain the Fork token; keep evidence private and outside Git.
