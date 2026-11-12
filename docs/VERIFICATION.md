# Verification · v0.1.0

验证日期：2026-11-12。执行环境：Linux x86_64 云容器，无 DISPLAY/Wayland 图形会话。
Rust 1.99.1；GPUI 0.2.2；Codex CLI 0.167.0-alpha.4。完整依赖版本见 Cargo.lock。

## 已通过

- `cargo fmt --check`。
- `cargo test --locked --no-default-features`：**19 项测试全部通过**，其中 9 项库单元测试、10 项独立进程集成测试。
- `cargo check --locked --all-targets`：包含 GPUI 界面、输入组件、核心、示例及测试的全量类型检查通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- Python 测试模拟器和协议检查脚本的语法编译：通过。
- 8 个由实际 Rust payload builder 生成的消息，通过本机 Codex 生成的稳定 JSON Schema 校验。
- 原生 Linux 桌面可执行文件完成编译和链接，`--help` 成功退出。
- 使用真实 Codex 子进程的 `initialize` / `initialized` 握手成功。

### Linux 链接说明

容器只有运行时共享库，没有 `libxcb.so`、`libxkbcommon.so`、`libxkbcommon-x11.so` 的开发链接名。
因此标准 `cargo build` 的第一次链接失败。没有修改系统或安装额外软件：只在构建临时目录
为已有的系统 `.so.1` / `.so.0` 建立链接名，再通过 `cargo rustc -- -L native=...` 完成链接。
正常 Linux 开发机安装 README 中列出的开发包后应使用标准命令；本次不把该预期当作跨平台验证。

依赖解析还发现旧 `xattr` 与最新 `libc 0.2.191` 不兼容，已在交付的 Cargo.lock 中降到
`libc 0.2.186`，并在这一锁定依赖集合上重跑最终检查。
上游 `proc-macro-error2 2.0.1` 有 Rust future-incompatibility 提示，不是本项目 Clippy 错误。

## 测试覆盖

单元：请求/通知/响应分类、错误响应、空结果、ID 类型、无 jsonrpc 字段、审批选择限制、
请求字段、Unicode 增量、最终快照去重、其他线程事件过滤、中断完成、断线清理、帧大小和 EOF。

独立 Python 进程集成：

1. 初始化顺序、线程创建、双向 ID 复用、接受/拒绝、重复点击、流式文字、diff、重复 turn。
2. 正常中断：等待 `turn/completed`。
3. 未实现的服务端请求得到 `-32601`，不会无响应卡死。
4. 后端意外退出清理状态。
5. 坏 JSONL 帧使连接显式失败。
6. 可重试错误通知不会被误当成 turn 结束。
7. 发送后立刻中断，在获得 turn ID 后执行。
8. 子线程的 `serverRequest/resolved` 也能移除审批，陈旧点击不发响应。
9. 关闭 Controller 给后端 EOF 清理时间，并等待子进程回收。
10. 重复使用未决审批 ID 的异常服务端被关闭连接，防止旧 UI 上下文批准新操作。

## 被环境阻塞

真实 Codex 的 `thread/start` 返回 `-32603`。服务端原始错误核心为：

```text
failed to load AGENTS.md instructions for environment `local`
fs sandbox helper failed
app-server socket directory must be a user-owned directory with mode 0700
```

这是运行容器的 App Server sandbox/helper 环境限制。没有绕过它、关闭沙箱、改权限或把后端
换成真实执行来掩盖问题。结果是：**真实后端建线程和真实模型任务闭环尚未通过**。

## 未运行/未验证

- 无图形桌面，未对真实 GPUI 窗口进行视觉、鼠标、滚动、IME 或剪贴板人工验收。
- 未用真实模型执行命令、编辑文件或触发审批。
- 未在 macOS/Windows 构建，未验证各 OS 的全部子进程树退出行为。
- 未测试账号登录、历史分页、额外权限/结构化问答，它们也不属于当前实现。
- 未打包签名/发布安装包。交付的是源码 ZIP，不是已签名的桌面产品。

## 本地最短验收路径

1. 装好 Rust、Codex CLI 和对应 OS 的图形构建依赖。
2. 在独立测试仓库用 `--read-only` 启动，发一句“只解释目录，不修改文件”。
3. 查看流式回复、线程 ID、诊断；发送后快速按 Esc、重复点击发送/审批，确认不会重复执行。
4. 在测试仓库中按需验证 workspace-write 和审批行为，注意 on-request 不等于每次写入都审批。
5. 用复制的线程 ID 运行 `--resume`；历史可能是后端返回的部分历史。
