# Remote relay / 远程中继

Home codeg establishes an outbound WSS tunnel to your VPS. iOS connects to the
VPS over HTTPS/WSS; requests travel through the multiplexed tunnel into codeg's
existing web router. No home public IP, port forwarding or VPN is needed.

家庭 codeg 主动连接自托管 VPS，手机访问 VPS 地址；桌面程序和独立 codeg-server 都支持。

## 1. Deploy / 部署

Download the matching `codeg-relay-linux-x64.tar.gz` or
`codeg-relay-linux-arm64.tar.gz` and its `.sha256` from
[Releases](https://github.com/PanAndDuck/codeg-fork/releases). The native musl
binaries are static; the kit includes both deploy scripts, unit and README files.
On a Linux VPS with systemd, use your real server address instead of the example:

```bash
sha256sum -c codeg-relay-linux-x64.tar.gz.sha256
tar -xzf codeg-relay-linux-x64.tar.gz
cd codeg-relay-linux-x64
sudo bash deploy/install.sh --binary ./codeg-relay \
  --port 18443 --public-host relay.example.com
```

Open that TCP port in your provider firewall. Add `--open-firewall` only if you
want the installer to manage a UFW rule. Save the generated agent key and the
leaf certificate SHA-256 fingerprint securely. For your own certificate or
manual CLI usage, see the [deploy guide](../../crates/codeg-relay/deploy/README.zh-CN.md).

部署前后核对已有业务服务、监听和防火墙规则，确保原业务不受影响。安装器只管理自己的
目录、用户、systemd unit 和明确要求的防火墙规则；不要把自己的文件放入其受管目录。
完整卸载：`sudo bash deploy/uninstall.sh`；先备份 key/cert 才能保留身份。

## 2. Connect home codeg / 配置家庭端

Open **Settings → Web service → Remote relay**. Start the web service, enter
the public URL (for example `https://relay.example.com:18443`), agent key and
fingerprint, test the connection, then enable/save. Check that status becomes
**Connected**. For a self-signed certificate, compare the displayed fingerprint
with the VPS output over a trusted channel before trusting it.

在“设置 → Web 服务 → 远程中继”填写 URL、agent key、指纹，测试并启用。Web 服务必须运行。
首次信任指纹需要人工核对；证书更换后也要重新核对，不能盲目接受变化。

For codeg-server, environment settings take precedence and make relay UI fields
read-only. Put real values in a private environment file, never in source control:

```bash
export CODEG_RELAY_ENABLED=true
export CODEG_RELAY_URL=https://relay.example.com:18443
export CODEG_RELAY_KEY='<agent-key-from-deployment>'
export CODEG_RELAY_FINGERPRINT='<64-lowercase-hex-leaf-DER-SHA256>'
# Optional, if the public URL differs:
export CODEG_RELAY_PUBLIC_URL=https://relay.example.com:18443
```

The relay **server** itself reads `CODEG_RELAY_AGENT_KEY` or `--agent-key-file`;
the home **client** reads `CODEG_RELAY_KEY`. These are the same agent credential,
but neither is the user's web-service token.

## 3. Pair iOS / 手机配对

Install [codeg-ios-fork](https://github.com/PanAndDuck/codeg-ios-fork). Reveal the
relay pairing QR in codeg and scan it in iOS, or copy/paste the revealed pairing
code. It includes public URL, **home web token** and certificate fingerprint.
Test and save the server, then open a conversation and check streaming replies.

二维码含真实访问凭据，仅在配对时显示；不要发布截图或配对码。agent key 不可填到手机的
web token 字段。切换官方身份包与并存 Codeg-Fork 时，需使用各自数据库对应的新配对码。

## Trust and troubleshooting / 信任边界与排查

- The relay terminates TLS: its administrator can see HTTP content and the web
  token. Use a VPS you trust. This is not end-to-end encryption through the relay.
- Only one home agent is active per relay; another authenticated agent replaces
  it. Disable the old client's relay before handing over to another install.
- `GET /__codeg_relay/v1/health` reports public health. Home offline returns
  `503 relay_agent_offline`; valid agent authentication adds connection metrics.
- Wrong key gives an auth error; pin mismatch fails closed. Confirm URL, agent
  key, trusted fingerprint and VPS firewall. A missing web service leaves the
  client waiting. Do not disable TLS validation to repair a mismatch.
- Self-signed certificates work with pinning in the clients. A browser needs
  its own certificate trust setup; use a public certificate for ordinary browsers.
- TLS supports HTTP/1.1. Connections/streams are bounded; keepalive and reconnect
  help recover outages but do not promise uninterrupted turns on every network.

中继管理员可读取流量，应保护服务器和证书私钥。一个 relay 只服务一个活动家庭端；
连接故障先检查 Web 服务状态、凭据、pin 和端口，再查看不含凭据的日志。
