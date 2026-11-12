# Coding Agent Desktop · v0.1.0

[![Cross-platform CI](https://github.com/smithlee200001/le-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/smithlee200001/le-agent/actions/workflows/ci.yml)

独立的 **Rust + GPUI** 桌面 Coding Agent 最小实现，以 **Codex App Server** 为执行后端。
不是 TreeLens 的补丁，不依赖原项目，不含 Git 推送或发布步骤。

## 实现了什么

- 真正的 GPUI 原生窗口：会话、流式回答、命令输出、统一 diff、审批卡片、诊断日志。
- 独立子进程 `codex app-server`；后台 Tokio runtime 处理 stdio JSONL，UI 不直接做进程 I/O。
- `initialize` → `initialized` → `thread/start` 或 `thread/resume` → `turn/start`。
- 双向 RPC：分别识别 response、notification 和 **服务端带 ID 的 request**。
- 命令/文件变更审批：接受一次或拒绝；保留原始数字/字符串 request ID；不自动审批。
- `turn/interrupt`，包括在 turn ID 尚未返回时排队取消。收到 `turn/completed` 才结束运行状态。
- ID 对齐的增量 reducer；恢复已有线程；进程退出/坏 JSON/超时的显式错误。
- 有界消息队列、8 MiB 单帧上限、45 秒 RPC 响应超时、5 秒写入超时、重复点击保护。
- UI 关闭时关闭 stdin，最多等待 2 秒让后端清理，再终止并回收该子进程。
- 可独立测试的 headless 核心、模拟 App Server、真实 CLI schema 校验脚本。

## 快速运行

准备：Rust stable（本次实际使用 1.99.1）、Codex CLI，以及可用的桌面图形会话。
先按 [Codex 官方安装说明](https://developers.openai.com/codex/cli/) 安装并完成登录：

```sh
codex --version
codex login
cargo run --locked --bin coding-agent-desktop -- --workspace /absolute/path/to/your/repository
```

工作目录必须存在。默认继承本机 Codex 的账号、模型配置及环境；不要在不信任的仓库里随意运行。
应用本身不读取、显示或保存密码。界面中发送的任务由你的 Codex 账号执行，可能产生相应额度/费用。

```sh
# 指定 CLI 路径、模型或恢复线程
cargo run --locked --bin coding-agent-desktop -- \
  --workspace /absolute/repo --codex /absolute/path/to/codex \
  --model YOUR_MODEL_ID --resume THREAD_ID

# 只读工作区
cargo run --locked --bin coding-agent-desktop -- --workspace /absolute/repo --read-only
```

不传 `--model` 就使用 Codex 配置，不硬编码可能不可用的模型名称。
复制线程 ID 后，可使用 `--resume` 在下次启动恢复。
Ctrl/⌘ + Enter 发送；Esc 中断；审批卡片显示服务端原始上下文。
**当前输入框是单行，粘贴的换行会转换为空格。** 不适合直接粘贴需保持缩进的代码块。

### 平台依赖

- **macOS**：需要 Xcode Command Line Tools；GPUI 使用系统图形后端。
- **Linux**：需要 X11/Wayland 桌面和可用 GPU/Vulkan 驱动。Debian/Ubuntu 常见构建依赖：
  `build-essential pkg-config libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig1-dev libfreetype6-dev libssl-dev libvulkan-dev`。
- **Windows**：需要 Visual Studio 的“使用 C++ 的桌面开发”工作负载和 Windows SDK，使用 Rust MSVC 工具链。CI 会原生编译和测试；真实窗口、IME、剪贴板和 Codex 账号闭环仍需在桌面上验收。

GPUI 固定为 crates.io **0.2.2**，交付了完整 `Cargo.lock`。必须使用 `--locked`。
锁文件将 `libc` 固定在 **0.2.186**，避免旧 `xattr` 传递依赖与移除 `ENOATTR` 的新 libc 不兼容。
不要不经测试就运行 `cargo update`。

## 安全边界

默认 `sandbox="workspace-write"`、`approvalPolicy="on-request"`、`approvalsReviewer="user"`。
这表示 **Codex 根据它的策略请求审批**，并非每条命令或每次文件写入都会先弹窗。
如需保守试用，请加 `--read-only` 并使用无敏感内容的测试仓库。

- UI 从不自己调用 shell 执行模型文本；执行和沙箱由 Codex 后端负责。
- “独立进程”是生命周期/故障隔离，**不是额外的 OS 沙箱**。
- UI 只支持命令和文件审批，不支持持久化授权、会话级授权、网络策略修订。
- 未实现的服务端请求（例如动态工具、额外权限、结构化问题）返回 `-32601`，不假造答案、不默许权限。
- 连接失败不会自动重试或重新发送任务，避免重复执行有副作用的操作。
- 关闭窗口会请求后端退出；强杀兜底只能保证回收直接子进程，不能保证所有孙进程在异常情况下都退出。
- 后端 stderr 可能包含项目路径和诊断信息；仅在本地内存显示，不主动上传日志。

## 目录

```text
src/protocol.rs       JSONL 消息分类、ID、请求/审批 payload
src/engine.rs         子进程、双向 RPC、超时、取消、审批队列
src/state.rs          UI 无关的事件 reducer
src/ui.rs             GPUI 主窗口与事件桥
src/text_input.rs     Unicode/IME 单行输入组件
src/bin/smoke.rs      真实 CLI 握手/建线程诊断，不发起模型 turn
examples/protocol_samples.rs  输出真实 Rust payload
scripts/check_protocol.py    用本机 CLI 生成的 schema 校验 payload
tests/transport.rs   与独立模拟进程的端到端测试
tests/mock_app_server.py    严格 stdio 模拟服务端
docs/ARCHITECTURE.md 架构、协议与扩展点
docs/VERIFICATION.md 本次实际验证结果和限制
```

## 持续集成

[Cross-platform CI](https://github.com/smithlee200001/le-agent/actions/workflows/ci.yml)
在每次 push、pull request 和手动触发时并行运行 Linux、macOS、Windows 原生 runner。
固定 Rust 1.99.1 和 Cargo.lock，执行格式检查、Python 语法检查、headless 核心/子进程测试、
全部桌面 targets 检查/测试、Clippy、桌面可执行文件编译与 `--help` 冒烟测试。
Windows 不再跳过 transport 测试；三平台都通过显式 Python 解释器启动模拟后端。
本地可通过 `PYTHON` 环境变量指定解释器路径；未设置时 Unix 使用 `python3`，Windows 使用 `python`。

成功的运行会保留 7 天未签名的开发版二进制 artifact（按 OS/架构命名），不是安装包。
Linux/macOS 从 ZIP 解压后如丢失执行位，可运行 `chmod +x coding-agent-desktop`。
CI 不需要 Codex 账号或模型密钥，不启动图形窗口、不调用真实模型；绿色 CI 只覆盖上述自动检查。

## 开发检查

```sh
cargo fmt --check
cargo test --locked --no-default-features   # 核心 + Python mock；无需 GPU/账号
cargo check --locked --all-targets          # 包含真实 GPUI UI 的类型检查
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --bin coding-agent-desktop

# 可选：需要 Python jsonschema，使用已安装的 Codex CLI 生成当前 schema
python3 -m pip install jsonschema
python3 scripts/check_protocol.py /path/to/codex

# 真实 CLI 诊断：会创建一个只读线程，但不会发送 prompt、调用模型或执行工具
cargo run --locked --no-default-features --bin agent-smoke -- /path/to/codex
```

### 无账号演示

在有图形桌面的 macOS/Linux 上，可以把测试模拟器当作后端启动界面：

```sh
cargo run --locked --bin coding-agent-desktop -- \
  --workspace "$PWD" --codex "$PWD/tests/mock_app_server.py"
```

需要 Python 3。发送普通文字会看到模拟审批、流式回答和 diff，**不会执行真实命令或修改工作区**。
发送 `interrupt` 可演示中断按钮。模拟器仅用于开发测试。

## 明确未实现

这不是 Codex Desktop 或 Zed 的完整克隆。尚无多工作区/线程列表、分页历史加载、自动重连、账号登录 UI、模型选择器、多行编辑器、Markdown/代码高亮、文件树、交互式终端、结构化工具问答、图片输入、文件附件、diff 撤销和打包签名。

`--resume` 会显示后端内联返回的历史；新版协议可能只给一部分历史。缺失的分页历史不等于没有历史，本版不会请求历史分页。
对话最多保留 2000 个显示项目，单项目文本最多 256 KiB；diff 同样有显示上限。它们只限制 UI，不修改 Codex 保存的历史。

本次可复现的验证边界请先读 [VERIFICATION.md](docs/VERIFICATION.md)。
