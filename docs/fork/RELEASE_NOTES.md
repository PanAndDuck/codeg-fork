# codeg-fork 0.34.0-fork.1

Based on spacering-net/codeg v0.34.0 · Apache-2.0 · Modifications by PanAndDuck.
基于官方 v0.34.0，保留 Apache-2.0 许可；这是独立 fork 的正式发行。

## 我们更新了什么

- 内建远程中继：家庭桌面/服务器主动连接自托管 VPS，支持 TLS 指纹固定、多路复用隧道、状态显示和 iOS 二维码/配对码；家中无需公网 IP、端口映射或 VPN。
- 新增 codeg-relay 服务和 systemd 安装/卸载工具，提供 Linux x64/arm64 静态包。
- 修复 WebSocket 写入背压死锁，加入心跳和停滞检测；认证采用常量时间比较、按 IP 限流和 TCP keepalive/NODELAY。
- 修复 ACP 回合完成顺序，最终 delta 不再晚于 turn_complete。
- 提供 updater-off 自托管构建、备份替换/回滚/同步脚本、可选 Codeg-Fork 并存 flavor 和隔离 E2E harness。

## What's new

- Built-in outbound remote relay with pinned TLS, multiplexed streams, status UI and iOS QR/copy pairing; no home public IP, port forwarding or VPN required.
- Standalone codeg-relay server, systemd deployment kit and static Linux x64/arm64 assets.
- WebSocket writer deadlock fix, heartbeats/stall detection, constant-time auth, per-IP limiting and TCP connection tuning.
- ACP queued updates are applied before turn completion, preserving final-delta ordering.
- Updater-off selfhost tooling, backup/swap/rollback/sync scripts, optional isolated Codeg-Fork flavor and deterministic E2E tooling.

[Detailed changes / 完整变更](https://github.com/PanAndDuck/codeg-fork/blob/main/docs/fork/CHANGES.md) ·
[Relay guide / 中继指南](https://github.com/PanAndDuck/codeg-fork/blob/main/docs/fork/RELAY.md) ·
[Companion iOS app](https://github.com/PanAndDuck/codeg-ios-fork)

## Downloads / 下载

Use the assets below: macOS arm64/x64 `.dmg` or `.app.tar.gz`; Windows x64/arm64
NSIS `setup.exe`; Linux amd64/arm64 `.deb`, x86_64/aarch64 `.rpm`, amd64 `.AppImage`.
`codeg-server` packages cover macOS arm64/x64, Linux arm64/x64 and Windows x64.
`codeg-relay-linux-{x64,arm64}.tar.gz` includes the binary and deploy files.
Docker: `ghcr.io/panandduck/codeg-fork:0.34.0-fork.1` (amd64/arm64).

Server and relay downloads include `.sha256`: run `sha256sum -c <file>.sha256`
(macOS: `shasum -a 256 -c <file>.sha256`). Checksums detect corruption; no updater
`.sig` files or `latest.json` are published. 校验值用于完整性核对，不代表签名认证。

## Installation notes / 安装须知

- **macOS packages without Apple credentials are ad-hoc signed and NOT notarized.** First open via right-click → Open; if necessary, after verifying the download, run `xattr -dr com.apple.quarantine /Applications/codeg.app`. 无 Apple 证书的包未公证，首次请右键打开或手动移除隔离标记。
- macOS computer use reports **unavailable in unsigned builds**; the helper also refuses direct invocation. Team-pinned trust remains mandatory in certificate-backed builds. 无证书包禁用电脑控制，其余工作台/中继功能可用。
- Windows SmartScreen: **More info → Run anyway** for a trusted download. Windows 包未附加代码签名。
- The in-app and server self-updaters are disabled. Download a new fork release to update. 更新器已关闭，请手动下载新版。
- Desktop assets use the **same identity and data locations as official codeg**. Installing over it keeps existing data; no new DB migrations were added relative to v0.34.0, so switching back to that official version is supported. Keep backups. 下载包沿用官方身份，不是并存 Codeg-Fork 包。
- After changing signatures, re-grant keychain **Always Allow** and macOS privacy permissions where applicable. 切换签名后按系统提示重新授权钥匙串与隐私权限。
- Deploy your own trusted relay, enable Settings → Web service → Remote relay, then pair iOS. The relay terminates TLS and can see web tokens; it supports one active home agent.
