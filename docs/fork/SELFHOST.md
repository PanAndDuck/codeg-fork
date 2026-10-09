# 正式身份自托管

本流程将自编译的 `app.codeg` 安装到 `/Applications/codeg.app`，复用正式数据目录和钥匙串。正式切换会中断应用内正在运行的编排会话。

## 实施约定

- `swap.sh` 做预检、应用副本、SQLite 在线备份、其他数据快照与 manifest；全部完成后才允许退出当前应用。
- 默认使用一次性 launchd job 运行独立 worker；worker 的代码和事务状态保存在备份目录，不依赖应用进程存活。
- worker 先完整复制并校验 `.new`，再把旧应用移动到同一文件系统的恢复位置，最后 rename 新应用。任一步失败均恢复旧应用。
- rollback 默认只还原应用；数据必须通过 `--restore-data` 明确指定。回滚前也备份当前状态。
- `--dry-run` 不建立目录、不读取真实数据库、不加载 job、不退出应用；仅打印计划和只读预检。
- 测试使用原生假 app、独立 bundle id、临时数据与 3097。`--inline` 仅允许完整隔离的测试路径。

实现与测试记录见任务报告 `codeg-fork-plan/reports/R2.md`。完整使用说明随脚本验收补充。
