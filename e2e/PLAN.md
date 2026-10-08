# E0 harness implementation plan

Spec: `/Users/you/Projects/codeg-fork-plan/tasks/E0.md`, PLAN §2 and §4.
Only test tooling lives here; Python standard library, no real LLM calls.

1. Mock ACP: subprocess contract tests for initialize/new/load, deterministic
   stream/tools, cancellation and failure; implement newline JSON-RPC with a
   reader independent of the prompt worker. Add permission/question/plan cards.
2. Environment: `env-up.sh` / `env-down.sh` delegate to a Python lifecycle owner.
   Clear inherited CODEG state, use C1's wrapper and the local release relay;
   record PID, command/start identity and temporary roots before polling.
   Refuse occupied/unsafe ports; clean up failed starts and normal shutdown.
3. Client/registration: pinned HTTPS + minimal RFC6455 stdlib client; register
   local binary using API, enable it, create folder/conversation through relay.
4. Fault proxy: TCP pass-through and TLS termination with separate certificate;
   bounded HTTP/WS parsing, loopback control API, fault counters for evidence.
   Test cancellation, fragmentation, HTTP preservation and one-shot faults.
5. Run real relay smoke: ordered 50-line turn, >1 MiB live snapshot, disconnect
   and cursor replay without missing/duplicate content; exercise content faults.
6. Read-only review, final tests, teardown/port/official PID checks, commits and
   Chinese E0 handoff report with exact E1a/E1b commands and limitations.

Checks: `python3 -m unittest discover -s e2e/tests -v`, shell syntax,
`python3 e2e/smoke.py --name e0-smoke`, `git diff --check`.
The Rust/UI production suites are integration-owned; this task builds real
server/relay binaries and tests the actual wire boundary without editing them.
