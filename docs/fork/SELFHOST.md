# 正式身份自托管

此流程将自编译的 `app.codeg` 安装到 `/Applications/codeg.app`，继续使用正式实例的数据、钥匙串服务名称和 `codeg://` 协议。应用退出后，运行在其中的编排会话会断开；一次性 launchd worker 继续完成替换和验证。

## 前提与构建

需要 macOS 图形登录会话、Bash、Python 3.9+、codesign、ditto、rsync、lsof 和 Xcode Command Line Tools。执行用户必须能写入 `/Applications`，应用卷与备份卷各至少有 5 GiB 可用空间，大型数据另留足备份空间。不要通过 `sudo` 运行：job 属于当前用户的 `gui/$UID`。

在已经验收的 fork checkout 中构建正式身份的暂存包：

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
scripts/build-selfhost.sh --stage
scripts/selfhost/swap.sh --dry-run --staged "$HOME/Applications/codeg-staged/codeg.app"
```

`build-selfhost.sh` 由正式身份构建任务提供；`build-fork.sh` 生成独立的 `Codeg-Fork.app`，不能作为正式身份替换源。此流程要求已有正式 app 和 `codeg.db`；空白机器首次安装请先用官方安装器初始化，再执行替换。

预检检查暂存包和当前包的签名、bundle id、主程序名称、空间和路径。新包必须带 `CodegBuildCommit`，且其 commit/version 和 cdhash 能区分于当前包。脚本拒绝重叠路径、残留 `.new` 和非完整隔离的 `--inline`。`--dry-run` 只读取 app 元数据/签名与空间，打印所有计划动作；不读取真实数据库，不建立日志或任何目录。

## 首次接管与升级

确认暂存包和 dry-run 输出后执行：

```bash
scripts/selfhost/swap.sh --staged "$HOME/Applications/codeg-staged/codeg.app"
```

主脚本完成备份后输出 `SUBMITTED`、job label、日志和 status 路径。`SUBMITTED` 只代表交接成功；完成标志是日志的 `FINAL OK` 和对应 `jobs/<id>/status` 的 `OK`。可从外部 Terminal 观察输出中给出的精确日志：

```bash
tail -f "$HOME/codeg-backups/logs/swap-<id>.log"
cat "$HOME/codeg-backups/jobs/<id>/status"
```

每次操作以 `umask 077` 新建私有目录、manifest 和日志；ditto/rsync 副本保留原权限，由外围私有目录限制访问。已有备份不会覆盖：

- `apps/codeg-<version>-<authority>-<id>.app`：ditto 完整应用副本，保留签名。
- `data/<id>/data/codeg.db`：SQLite online backup API 快照，可安全读取运行中的 WAL 数据库；随后 `integrity_check`。
- `data/<id>/data/`：其余 app data 的 rsync，排除根目录的 `codeg.db`、`-wal`、`-shm`。
- `data/<id>/home/`：`~/.codeg` 的 rsync。
- `manifests/<id>.json`：版本、签名 authority/cdhash、commit、大小、数据库 SHA-256、路径、备份完成标志和 Web 自动启动判定；不记录 token。
- `jobs/<id>/`：本次 worker 代码副本、一次性 launchd plist 和最终 status；`logs/` 保存步骤与失败原因。

备份完整且写盘后，主脚本通过 `launchctl bootstrap gui/$UID` 启动独立 worker。worker 等 5 秒，发出 quit Apple Event；最多 45 秒后升级 TERM，再等 15 秒后升级 KILL，每次均记录并以 `proc_pidpath` 精确限定 `/Applications/codeg.app/Contents/MacOS/codeg`。如果另一路径正在运行相同 bundle id，拒绝发送 quit。不会按名称匹配 `codeg-mcp`、helper 或 Codeg-Fork。

worker 在安装卷先复制和校验 `codeg.app.new`，再把旧包 rename 到同卷 `.replaced-<id>`，将 `.new` rename 到正式路径。最终 rename 是原子的；两次 rename 之间存在短暂路径空窗。旧包在验证成功前一直保留。安装后移除 quarantine、执行 `lsregister -f`，通过 `open -a` 启动。

最多等待 60 秒核验精确主程序进程、已签名包的 `CodegBuildCommit`/cdhash；备份中的 `web_service_auto_start` 为 true/1/yes 时，还要求指定端口由这个进程 LISTEN（默认 3080）。不会请求 API 或读取 token。`--no-relaunch` 可只安装并校验包，日志明确标记运行验证待完成。

升级后执行同一构建、dry-run、swap 流程。正常结束自动卸载一次性 job，保留 plist/日志用于审计。锁位于 `~/codeg-backups/.lock`；出现陈旧锁时先核对日志、status 和 `launchctl print gui/$UID/<label>`，确认没有 worker 后再处理，脚本不会盲目清锁。若整机断电或 worker 被 SIGKILL，EXIT 自动恢复不能运行，应先检查 `.new`/`.replaced-*` 和 manifest，保留副本后人工恢复。

## 回滚

默认只还原应用，不覆盖升级后产生的会话或设置：

```bash
scripts/selfhost/rollback.sh --dry-run --to "$HOME/codeg-backups/apps/<backup>.app"
scripts/selfhost/rollback.sh --to "$HOME/codeg-backups/apps/<backup>.app"
```

省略 `--to` 时，选择最新的已完成备份且原 app 没有 `CodegBuildCommit` 的 upstream/original 包；找不到会要求明确指定。每次手动回滚也先备份当前 app 和数据，再交由 launchd，退出/替换/重启验证与 swap 相同。

只有需要撤销数据迁移或设置变化时才显式恢复快照：

```bash
scripts/selfhost/rollback.sh --to "$HOME/codeg-backups/apps/<backup>.app" --restore-data '<snapshot-id>'
```

**这会删除所选快照之后的数据和设置。** worker 停止应用后，通过 `rsync --delete` 恢复 data/home，并移除旧 WAL/SHM。操作前的新备份可用于找回数据；数据库快照先执行完整性检查。应用和文件树备份发生于运行期间：SQLite 内部一致，其他可变文件并非跨目录同一瞬间的事务快照。

替换、注册、启动或验证失败会自动还原旧包；如果显式恢复数据过程中失败，也还原操作前的数据快照。失败的新包保留为 `codeg.app.failed-<id>`。日志中 `AUTO-ROLLBACK OK` 表示旧应用恢复成功，但本次升级 status 仍是 `FAIL`。若自动恢复也失败，日志保留准确位置，不删除备份。

## 钥匙串与电脑控制权限

正式身份继续使用原钥匙串服务；自编译签名与官方 Developer ID 签名不同，macOS 可能重新询问访问钥匙串。确认请求来自刚安装的 codeg 后选择 **Always Allow（始终允许）**。脚本不会导出、删除或自动授权钥匙串项目。

Computer Helper 的签名发生变化后，首次电脑控制可能需要重新授予辅助功能和屏幕录制权限。在“系统设置 → 隐私与安全性 → 辅助功能 / 屏幕与系统音频录制”中重新允许当前正式包的 helper；若旧条目无效，移除旧条目后重新触发请求，并按系统提示重启应用/helper。脚本不会重置 TCC 数据库或代替用户点授权。

## Relay 交接与 iOS 重新配对

一个 relay agent key 同时只允许一个 home 连接；新连接会挤掉旧连接，双方自动重连会反复争抢。正式构建接管前，在 **Codeg-Fork → 设置 → Web 服务 → 远程中继** 中关闭“启用中继”并保存。可保留 Fork 应用运行；替换脚本不会修改或退出它，也不会修改 relay 服务。

随后在正式 codeg 中启动 Web 服务（默认 3080），按需要开启自动启动，填写已保存的 relay URL、agent key 和证书 fingerprint，测试并启用，确认 Connected。正式数据库与 Fork 数据库分离，Fork 的 token、配对和 relay 设置不会自动搬过来。不要将 agent key 作为 iOS 的 Web token。

在正式实例显示“中继”配对二维码或复制配对码，用 iOS 的服务器添加/编辑入口重新导入；核对 URL、证书 pin 和正式实例的 token，测试连接后保存。打开会话列表，验证一次流式回复、后台恢复和网络切换。配对后隐藏二维码；不要把 token 或配对码放进日志。切回 Fork 时同样先关闭正式实例的 relay，再启用 Fork 并恢复对应的 iOS 配对。

## 同步 upstream

在干净的专用工作树中执行，脚本以本地 `fork/main` 为起点：

```bash
scripts/selfhost/sync-upstream.sh vX.Y.Z
```

脚本 fetch upstream tags，创建 `sync/<tag>`，merge 指定 tag；有冲突立即列出文件并停止，需人工解决和提交。成功合并后顺序执行 pnpm install/lint/test/build、Rust desktop/server/mcp check/test/clippy、tunnel/relay crate tests；E2E harness 存在时启动独立本地环境并执行 smoke 和 completion probe，最后运行 `build-fork.sh`。E2E 使用 3098、18473–18475，测试 home/data 为临时目录，失败也清理自己启动的环境。任何检查失败都停止后续构建，不自动解决冲突或 push。

先验收 Codeg-Fork 构建，再执行 `build-selfhost.sh --stage`、dry-run 和 swap。日志在 `~/codeg-backups/logs/sync-*.log`。已有同名 sync 分支时脚本拒绝重跑；在该分支修复后按日志补跑失败项和后续检查，或为新的 tag 启动下一次同步。

## 隔离测试与路径覆盖

| 环境变量 | 默认值 |
| --- | --- |
| `CODEG_SWAP_APPS_DIR` | `/Applications`（当前 app 为其中的 `codeg.app`） |
| `CODEG_SWAP_DATA_DIR` | `~/Library/Application Support/app.codeg` |
| `CODEG_SWAP_HOME_DIR` | `~/.codeg` |
| `CODEG_SWAP_BACKUP_DIR` | `~/codeg-backups` |
| `CODEG_SWAP_BUNDLE_ID` | `app.codeg` |
| `CODEG_SWAP_PORT` | `3080` |

`--inline` 仅供测试：必须同时隔离 app/data/home/backup，使用不同的 bundle id 和端口。自定义 bundle id 的 job label 自动使用 `app.codeg.selfhost.test.*` 前缀。不要为真实安装使用 `--inline`。

```bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
python3 scripts/selfhost/tests/test_selfhost.py
python3 scripts/selfhost/tests/test_sync.py
for script in scripts/selfhost/*.sh; do bash -n "$script"; done
shellcheck -x scripts/selfhost/*.sh
```

应用测试用临时原生 app、真实 ad-hoc 签名、SQLite WAL、3097（重复身份测试另用 18498）和真实 launchd；测试结束清理自己创建的进程与 job。同步测试使用本地临时 Git remote，pnpm/cargo/build/E2E 用受控替身验证调度和失败边界，不冒充产品完整检查矩阵通过。
