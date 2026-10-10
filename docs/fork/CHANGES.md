# What's new in this fork / 我们更新了什么

Based on **spacering-net/codeg v0.34.0**, Apache-2.0. Modifications by PanAndDuck.
This list comes from `git log upstream/main..HEAD`. Commit **titles** below are
stable lookup keys (`git log --all --grep='<title>'`); publication rewrites personal
metadata/history, so pre-publication commit hashes are deliberately omitted.

## Remote relay / 内建远程中继

The home desktop or codeg-server dials out to a VPS over WSS; public HTTP/WS
connections are multiplexed into the same in-process web router. Home needs no
public IP, forwarded port or VPN. Settings includes URL/key configuration,
certificate fingerprint trust, connection testing/status and explicit reveal/
copy of mobile pairing codes. TLS can use WebPKI or a pinned leaf certificate.

家庭端主动连接 VPS；设置页支持证书指纹、连接状态和二维码/配对码，手机无需直连家庭网络。

- `feat(relay): serve home codeg through an outbound pinned tunnel`
- `feat(relay): add frontend contracts and pairing helpers`
- `feat(settings): add remote relay configuration and certificate trust`
- `feat(settings): add consent-gated mobile pairing QR codes`
- `feat(settings): copy revealed pairing codes`
- `fix(relay): align frontend commands with named config arguments`
- `docs(relay): document home client configuration and trust`

## Relay server and tunnel / 中继服务与多路复用

`codeg-relay` provides TLS, agent authentication, bounded connections/streams,
heartbeats, safe logging, persistent self-signed certificates and systemd install/
uninstall tooling. `codeg-tunnel` extracts the existing framing/pump and adds a
bounded transport-independent mux with cancellation. Browser tunnel wire format
is retained; relay uses `codeg-relay.v1` at `/__codeg_relay/v1/agent`.

服务端提供独立 Linux 部署包、限流、证书持久化及受管防火墙回滚。一个 relay 同时只有一个
active agent；中继终止 TLS，管理员能看到 HTTP 内容和 web token，不是端到端加密。

- `refactor(tunnel): extract shared frame and pump crate`
- `feat(tunnel): add bounded transport-independent stream mux`
- `fix(tunnel): cancel sessions aborted before first poll`
- `feat(relay): add hardened TLS relay with authenticated mux sessions`
- `feat(relay): add isolated systemd deployment and owned firewall rollback`
- `build(relay): add static amd64 musl build and container smoke checks`
- `fix(relay): allow constrained Docker builds to lower concurrency`

## Reliability and authentication / 可靠性与认证

An independent WebSocket writer removes the bounded-queue self-deadlock;
heartbeats and write-stall detection bound failed connections. Auth uses a
constant-time token comparison, bounded per-IP failure limiting, TCP keepalive
and NODELAY. ACP completion now drains queued updates before `turn_complete`,
with a late-delta fence, so the final delta cannot follow completion.

修复背压死锁、失联无响应、token 比较时序问题和回合完成事件顺序；loopback 豁免认证失败限流，
真实客户端 IP 随隧道传入家庭路由。测试不能据此保证任意网络条件都不会断线。

- `fix(web): prevent websocket backpressure deadlocks and bound liveness`
- `feat(auth): add bounded per-IP authentication failure limiter`
- `fix(web): harden token auth and configure client connections`
- `fix(acp): apply queued updates before completing turns`

## Builds, handover and tests / 构建、交接与测试

The optional `Codeg-Fork` flavor isolates identity, data, ports and keyring names.
Published desktop assets instead use the official identity to preserve existing
data. Both fork release and selfhost builds disable the updater. Selfhost tooling
adds a build stamp, staged signing, detached swap/rollback with backups, and
upstream sync checks. The E2E harness provides a deterministic mock ACP agent,
isolated relay/server environments, fault injection and simulator evidence tools.

可选并存版本与正式身份包用途不同，见 [FLAVOR.md](FLAVOR.md) 和 [SELFHOST.md](SELFHOST.md)。
相对上游 v0.34.0 没有新增数据库迁移；relay 设置使用现有 app_metadata。

- `feat(flavor): isolate runtime identities and inherited state`
- `feat(ui): display fork branding and disable updater checks`
- `build(fork): add isolated desktop and server workflows`
- `fix(build): isolate inherited fork build environment`
- `fix(fork): build with the repo-pinned pnpm non-interactively`
- `feat(selfhost): disable upstream updates while preserving official identity`
- `feat(selfhost): build and stage a consistently signed macOS app`
- `fix(selfhost): stamp CodegBuildCommit into Info.plist`
- `test(selfhost): define isolated swap acceptance fixtures`
- `feat(selfhost): add detached transactional app swap and rollback`
- `test(selfhost): verify launchd recovery and document handover`
- `test(e2e): add isolated relay harness and scripted ACP agent`
- `test(e2e): verify relay server and desktop acceptance scenarios`
- `test(e2e): add E1b simulator fault evidence tooling`
- `fix(e2e): allow integration control ports through 18499`

## 0.34.0-fork.1 publication / 首次公开发行

Machine-specific paths, certificate identity/team, production service/domain/IP
examples are replaced with parameters or documentation addresses. Release CI
retains upstream's desktop/server matrix, adds static Linux relay x64/arm64 kits,
uses GHCR, disables updater manifests/signatures and stamps the build commit.
Without Apple credentials, macOS uses ad-hoc signing and disables computer use
on both sides of the helper boundary. Certificate-backed validation is preserved.

公开包无需上游 secrets；macOS 未公证包需手动首次打开，更新通过下载新 release。
独立的 iOS 应用与其变更见 [codeg-ios-fork](https://github.com/PanAndDuck/codeg-ios-fork)。
