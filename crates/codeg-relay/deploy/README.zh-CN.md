# codeg-relay 公网中继部署

`codeg-relay` 是独立 Rust 单文件服务。家庭 codeg 主动建立 WSS 隧道，手机通过
`https://公网IP:18443` 访问家庭服务。不需要家庭路由器端口映射，也不需要修改 VPS
上的 nginx。默认只监听一个 TCP 端口，TLS ALPN 仅 `http/1.1`。

Relay 终止 TLS，因此中继管理员能看到经过中继的 HTTP 明文和 codeg web token。
请使用可信 VPS，保护 root、agent key 和证书私钥。agent key 只用于家庭端建立隧道；
手机使用的是家庭 codeg 的 web token。TLS fingerprint 必须通过可信渠道核对。
这不是端到端加密代理，也不提供多个家庭租户：新 agent 会用 4001 `replaced` 替换旧连接。

## CLI

```bash
codeg-relay --version
codeg-relay gen-key
codeg-relay serve --listen 0.0.0.0:18443 \
  --agent-key-file /etc/codeg-relay/agent.key \
  --self-signed --public-host 203.0.113.10,relay.example.com \
  --data-dir /var/lib/codeg-relay
codeg-relay fingerprint --data-dir /var/lib/codeg-relay
codeg-relay fingerprint --tls-cert /path/to/cert.pem
```

- `--agent-key-file` 优先；未指定时读取 `CODEG_RELAY_AGENT_KEY`。key 至少 16 字符，
  不包含空白；`gen-key` 生成 32 随机 bytes 的无 padding base64url。
- 自有证书用 `--tls-cert cert.pem --tls-key key.pem` 替代 self-signed 参数。
- Self-signed cert/key 保存在 `<data-dir>/tls/{cert,key}.pem`，权限 0600，重启复用。
  更改 `--public-host` 不会自动轮换既有证书；轮换时备份并替换完整证书对，再更新客户端 pin。
- Fingerprint 第一行为 leaf DER SHA-256，64 个小写 hex；第二行为大写冒号分隔形式。
- `--insecure-plaintext` 只允许 loopback，仅供本地测试。
- `--max-conns` 默认/上限 512，`--max-conns-per-ip` 默认/上限 64，可以调小。
  计数包括 TLS 握手、HTTP 首部等待和 agent。拒绝连接另有 16 个有界 TLS 响应槽；
  槽耗尽时立即断开。Mux 同时最多 256 个 streams，heartbeat 15 s / idle 45 s。
- `RUST_LOG` 默认 `info`，允许 `codeg_relay=debug` 等覆盖；依赖库原始报文日志始终过滤，
  包括 `RUST_LOG=trace`。连接日志只记录 IP、stream ID、method、首个 path segment、
  bytes、duration 和固定 close reason。请勿在首段 path 放置敏感值。
- SIGINT/SIGTERM 后停止接收，关闭 agent，最多等待 5 s，然后取消剩余连接。

## 构建与安装

在仓库根目录执行：

```bash
crates/codeg-relay/scripts/build-linux-x86_64.sh
# 内存紧张的共享 Docker VM 可降低并发，默认 4 jobs：
CARGO_BUILD_JOBS=1 crates/codeg-relay/scripts/build-linux-x86_64.sh
```

脚本用 `docker run --rm --platform linux/amd64`、`rust:1-alpine` 构建
`x86_64-unknown-linux-musl`，整个 repo root 挂载解决 path dependency；Cargo registry
使用 named volume。生成 `dist/codeg-relay-x86_64-unknown-linux-musl` 和同名 `.sha256`，
并执行 static 检查、`--version` 和 3 秒 TLS loopback smoke。dist 不进入 Git。

将 binary、`.sha256`、本 deploy 目录一起复制到 Linux 服务器的暂存目录。需要已有
systemd、useradd、flock、ss；脚本不安装系统软件，不运行 apt，不操作其它 service。
Installer 只允许 1024–65535 的非数据库端口，避免接管共享服务端口。

```bash
sha256sum -c codeg-relay-x86_64-unknown-linux-musl.sha256
sudo bash deploy/install.sh \
  --binary ./codeg-relay-x86_64-unknown-linux-musl \
  --port 18443 --public-host YOUR_PUBLIC_IP
# 只有明确需要脚本开放 UFW 时，才添加 --open-firewall
```

安装路径：`/opt/codeg-relay/bin/codeg-relay`、`/etc/codeg-relay/relay.env`（root 0600）、
`/etc/codeg-relay/agent.key`（codeg-relay 0600）、`/var/lib/codeg-relay`（codeg-relay 0700）。
使用专用 nologin 用户和 systemd hardening；MemoryMax=128M、CPUQuota=50%、
TasksMax=256、LimitNOFILE=8192。内存/CPU 上限用于隔离共享 VPS；大量并发或恶意细碎帧
可能触及资源上限并触发服务重启，不保证 256 streams 能在 128 MiB 下满速运行。

重复执行同一安装命令会保留 agent key 和证书并更新/重启 relay；key 仅首次生成时打印。
脚本用 marker 拒绝接管同名的未管理目录、用户或 unit。不要将其它文件放入这三个专用目录。
未指定 `--open-firewall` 时完全不调用 ufw/iptables。
指定后，已有同端口规则保留；没有时执行 `ufw allow <port>/tcp comment 'codeg-relay'`，
仅确认新增后记录 `/etc/codeg-relay/ufw-port`。已有 deny/restrictive 规则需管理员自行评估，
脚本不会覆盖。存在受管理 firewall marker 时，换端口需先卸载。

## 家庭 codeg 与 iOS 配对

1. 在家庭 codeg 的 Relay 设置中填写安装输出的 URL、agent key 和 fingerprint，启用 relay。
2. 等待 connected；一个 relay 只能有一个 active agent。
3. 手机扫描家庭 codeg 生成的 relay QR，或填写同一 HTTPS URL、家庭 web token 和 fingerprint。
4. `GET /__codeg_relay/v1/health` 总是公开返回 status/version；只有有效 agent Bearer
   才显示 agent_connected/active_streams。普通请求在 agent 离线时返回 503 `relay_agent_offline`。
5. 自签证书浏览器需要单独建立信任；iOS / 家庭端按 fingerprint pin 验证。

## 共享服务器隔离检查

部署前记录现有服务状态、启动时间、网站响应、监听端口及防火墙规则。
每一步后确认现有服务不受影响：原服务无重启、原监听地址/进程不变，
只有 relay 端口及明确授权的防火墙规则发生变化。示例地址请替换为自己的服务器。

```bash
systemctl --failed
ufw status numbered
ss -ltnp
curl -s -o /dev/null -w '%{http_code}' https://example.com/
sudo bash deploy/install.sh --binary ./codeg-relay \
  --port 18443 --public-host 203.0.113.10
systemctl status codeg-relay --no-pager
sudo /opt/codeg-relay/bin/codeg-relay fingerprint --data-dir /var/lib/codeg-relay
sudo curl --cacert /var/lib/codeg-relay/tls/cert.pem https://203.0.113.10:18443/__codeg_relay/v1/health
```

不要修改或重启已有业务服务；按需由管理员单独开放 relay TCP 端口。
`systemctl daemon-reload` 会重读 unit 定义，不应重启已有服务。

## 完整回滚

```bash
sudo bash deploy/uninstall.sh
```

卸载停止/禁用 relay、删除 unit、daemon-reload；仅在 marker 及现有 rule comment 均匹配时
执行 `ufw delete allow <port>/tcp`，从不删除预存规则。外部修改过规则时停止并保留 marker
供人工核对。随后移除专用用户/组、三个专用目录（包括 key/cert）。再次卸载无副作用。
卸载前需要保留身份时，将 key/cert 备份到受保护的位置；完整卸载后重新安装会改变身份。

人工回滚命令如下；UFW 删除仅可用于确认由此次安装添加的规则，优先用脚本：

```bash
systemctl disable --now codeg-relay
ufw delete allow 18443/tcp
rm -f /etc/systemd/system/codeg-relay.service
systemctl daemon-reload
userdel codeg-relay
rm -rf /opt/codeg-relay /etc/codeg-relay /var/lib/codeg-relay
```

回滚后再次运行 baseline 命令比较，确认原业务无变化。
