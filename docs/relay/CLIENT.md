# Home relay client

The desktop app and `codeg-server` include the relay client. Enable the web
service, enter the relay address and agent key in **Settings → Web service →
Remote relay**, then save. Home codeg opens an outbound WebSocket; no extra
process, inbound port forwarding or public home IP is required.

The public relay terminates TLS and can see forwarded HTTP traffic. Use a
relay you trust. The relay agent key authenticates home codeg to the relay;
the separate codeg web token still authenticates every protected HTTP/WS
request from your phone/browser.

## Configuration

Settings persist transactionally in `app_metadata` under
`web_relay_enabled`, `web_relay_url`, `web_relay_key`,
`web_relay_fingerprint`, and `web_relay_public_url`. API responses expose only
`key_set`, never the stored key.

| Environment variable | Meaning |
| --- | --- |
| `CODEG_RELAY_URL` | Relay origin; `https://host:port`, `wss://host:port`, or `host:port` |
| `CODEG_RELAY_KEY` | Agent bearer key, at least 16 characters |
| `CODEG_RELAY_FINGERPRINT` | Optional SHA-256 of the leaf certificate DER |
| `CODEG_RELAY_PUBLIC_URL` | Optional address to use for phone pairing |
| `CODEG_RELAY_ENABLED` | `true`/`false` or `1`/`0`; defaults true when URL and key are available from env configuration |

Each supplied environment value overrides its saved setting. If any of these
variables is present, the source is `env` and settings updates return
`relay_config_from_env`. Empty fingerprint/public URL environment values clear
those overrides. Remove all relay environment variables to restore editable
saved settings. Invalid configuration appears as an error without preventing
the local web service from starting.

The URL can optionally include `/__codeg_relay/v1/agent`. Other agent paths,
credentials, queries and fragments are rejected. The default public URL is
the normalized HTTPS origin. Plain HTTP/WS is restricted to loopback hosts
and the connected peer is checked again. It is intended for local tests.

## Certificate trust

Without a pin, TLS uses the platform trust store and hostname verification.
With a pin, the client accepts the server only if SHA-256(leaf DER) equals
that pin; self-signed certificates and IP addresses are supported. The check
occurs before sending the HTTP upgrade or bearer key. Pins are 64 lowercase
hex characters; `sha256/`, `SHA256:`, colon-separated pairs and whitespace
are accepted and normalized.

Testing an untrusted self-signed certificate returns `tls_untrusted` and the
presented fingerprint. The diagnostic certificate probe sends no credentials
or application data. Compare that fingerprint against the relay operator's
trusted output before saving it. A mismatch is never automatically accepted.

Testing an already connected, identical configuration reuses the verified
live session. Testing other credentials/trust settings establishes a new
agent connection: under the relay's single-agent rule, a successful test can
replace an existing agent. Certificate rotation requires explicitly changing
the saved fingerprint.

## Commands (desktop and HTTP)

Requests use one named `config` argument, with camelCase fields inside it.
HTTP uses `POST /api/<command>`, bearer authentication and JSON; Tauri uses
`invoke(command, args)`. Responses use snake_case fields from PLAN §4.3.

```json
{}
```

Use the above for `get_relay_config` and `get_relay_status`.

`update_relay_config`:

```json
{"config":{"enabled":true,"url":"https://relay.example:18443","key":"replace-with-your-agent-key","fingerprint":null,"publicUrl":null}}
```

Omitted fields preserve their saved values. `fingerprint:null` and
`publicUrl:null` clear them; an empty key is rejected. For example,
`{"config":{"enabled":false}}` disables without changing the key or pin.
Saving restarts the client after a short 250 ms acknowledgement window.

`test_relay_connection`:

```json
{"config":{"url":"https://relay.example:18443","fingerprint":null}}
```

Omitting `key` uses the saved/effective key. A missing or null fingerprint
tests normal TLS trust. Result fields are `ok`, `fingerprint`, `rtt_ms`,
`error`, and `error_code`. Errors are `tls_untrusted`,
`fingerprint_mismatch`, `auth_rejected`, `unreachable`, `timeout`, or
`protocol`. Invalid input is rejected before dialing.

## Status and recovery

| State | Meaning |
| --- | --- |
| `disabled` | Relay disabled |
| `waiting_for_web_service` | Enabled; local web service stopped |
| `connecting` | Establishing TLS/authenticated WebSocket |
| `connected` | Accepting forwarded streams |
| `backoff` | Connection lost or dial failed; retry pending |
| `error` | Invalid configuration or authentication rejected |

Status includes connection time, ping/pong RTT, active/total stream counts,
the last error, remaining retry time and the observed certificate fingerprint.
State transitions emit `relay://status` immediately; counter updates emit at
most once per second. Counters are process-local and total streams include
admitted OPENs later rejected by local policy.

The tunnel pings every 15 seconds and closes after 45 seconds without incoming
traffic. Retry delays are 1, 2, 4, 8, 16, 30 seconds with ±20% jitter, resetting
after 60 seconds connected. HTTP 401/403 waits five minutes between retries;
correcting/saving the configuration retries promptly. Close code 4001 means
another authenticated agent replaced this instance.

The relay shares the fully layered local Router, including its auth limiter.
OPEN metadata must contain a literal client IP; client-supplied forwarding
headers do not change that identity. HTTP keep-alive and WebSocket upgrades
are supported. At most 256 streams overall and 64 per IP can remain open,
including upgraded sockets. Request headers have a 30-second deadline.

Stopping the web service cancels the tunnel and releases its Router. Desktop
exit and server SIGINT/SIGTERM do the same. Restarting the relay reconnects
automatically; an individual interrupted request may need client-side retry.

## Local verification

Unit/integration fixtures are inside `src-tauri/src/relay/tests.rs` and run
against in-process loopback TLS relays with generated certificates. They need
no external relay. Always use isolated `CODEG_HOME` and `CODEG_DATA_DIR` for
test/server runs. `scripts/fork-server-dev.sh` builds a fork server with fresh
temporary directories on 127.0.0.1:3091; pass relay environment values only
after clearing inherited application environment variables.
