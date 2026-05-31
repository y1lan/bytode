# bytode

> **BYTODE** = **B**ian(便) **Y**i(宜) **To**ken co**DE** —— 终端 Rust 编程助手

bytode 是一个终端编码代理（ReAct 循环 + DeepSeek + LSP + TUI），目前专注于 Rust 项目开发。工作在终端内部，通过类型化工具与代码库交互，**零 Shell 访问**。

## 安装

```bash
git clone https://github.com/y1lan/bytode.git
cd bytode
cargo install --path .
```

`~/.cargo/bin/bytode` 安装完成后即可全局使用：

```bash
bytode "帮我重构 src/main.rs"
```

### 依赖

| 依赖 | 用途 |
|------|------|
| `rust-analyzer` | LSP 诊断 (`get_diagnostics`) |
| `ripgrep` | 代码搜索 (`search_code`) |
| `git` | 版本控制工具 (`git_status` / `git_diff` / `git_log`) |
| Rust 2024 toolchain | 编译必需 |

### 环境变量

```bash
export DEEPSEEK_API_KEY="sk-..."
```

## 快速开始

### 交互模式 (REPL)

```bash
bytode
```

启动 ratatui TUI 终端，进入异步 REPL 循环（`tokio::select!` 并发处理键盘事件和 LLM SSE 流）。

特性：
- 列出 / 恢复历史会话（按最近修改时间排序）
- 实时流式输出；LLM 响应期间键盘仍可操作（滚动 / Ctrl+C 取消 / 输入缓冲）
- 工具调用显示参数摘要，如 `⟳ read_file(src/main.rs, offset=10)` 而非空括号
- 状态栏显示：耗时、会话成本（USD）、上下文用量、当前模式、工具数量、Git 分支
- 崩溃自动恢复终端（`catch_unwind` + `Terminal::restore()`）
- Markdown 渲染（粗体、斜体、代码块带 Rust 语法高亮、标题、引用、diff 着色）

### 单次任务 (One-shot)

```bash
bytode "在 src/main.rs 中添加一个问候函数"
```

单次任务模式自动加载已有会话，流式输出到 stdout。

### 指定模型

```bash
bytode --model deepseek-v4-flash
```

### 命令行参数

| 参数 | 简写 | 默认值 | 说明 |
|------|------|--------|------|
| `[TASK]` | — | — | 任务内容，省略则进入交互模式 |
| `--project` | `-p` | `.` | 项目根目录 |
| `--api-key` | — | `$DEEPSEEK_API_KEY` | DeepSeek API 密钥 |
| `--model` | — | `deepseek-v4-pro` | 模型名称 |

## REPL 命令

| 命令 | 说明 |
|------|------|
| `/exit` / `/quit` | 退出并保存会话 |
| `/model` | 查看当前模型 |
| `/model pro` | 切换到 `deepseek-v4-pro` |
| `/model flash` | 切换到 `deepseek-v4-flash` |
| `/plan` | 切换 Plan 模式（只读） |
| `/plan on` | 开启 Plan 模式 |
| `/plan off` | 回到 Build 模式 |

### 键盘快捷键

| 按键 | 功能 |
|------|------|
| `Enter` | 提交输入 |
| `Ctrl+C` | 取消当前 LLM 调用（流式输出中） / 退出（空闲时） |
| `Ctrl+T` | 切换工具侧栏 |
| `↑` / `PageUp` | 向上滚动历史（步长 5 / 20 行） |
| `↓` / `PageDown` | 向下滚动历史（步长 5 / 20 行） |

## 模式

### Build 模式（默认）

完整工具集，可读写文件、运行构建、操作 Git。Agent 发出 `write_file` 时弹出确认对话框。

### Plan 模式（只读）

禁用以下工具：
- `write_file`
- `git_status` / `git_diff` / `git_log`

其他工具（`read_file`、`search_code`、`get_diagnostics`、`cargo_check`、`web_search`、`cargo`）照常可用。切换模式时即时重建核心提示词中的工具列表。

## 工具列表

共 10 个类型化工具，统一实现 `Tool` trait：

| 工具 | 类别 | 可用性 | 超时 | 说明 |
|------|------|--------|------|------|
| `read_file` | 只读 | 始终 | 30s | 读取文件（行号 + offset/limit）或列出目录（含文件大小） |
| `write_file` | 修改 | 始终 | 10s | 原子写入（临时文件 + 重命名），diff 输出，自动 `didChange` 通知 LSP |
| `search_code` | 只读 | 始终 | 15s | `rg --json --line-number --no-heading`，支持 pattern 和 path |
| `cargo_check` | 构建 | Rust | 120s | `cargo check --message-format json`，支持 simple filter（errors/warnings/length） |
| `get_diagnostics` | 只读 | Rust | 5s | 从 rust-analyzer `publishDiagnostics` 缓存读取，支持 path/filter |
| `cargo` | 构建 | Rust | 120s | 白名单 Cargo 子命令：check/build/test/clippy/fmt/doc/bench/run/clean/update |
| `web_search` | 只读 | 始终 | 可配 | DuckDuckGo HTML 搜索（无需 API Key），支持代理 |
| `git_status` | 只读 | 始终 | 10s | `git status --porcelain`，支持 path 过滤 |
| `git_diff` | 只读 | 始终 | 15s | `git diff` / `git diff --cached`，支持 staged + path |
| `git_log` | 只读 | 始终 | 10s | `git log --oneline`，支持 count（1-100）和 path |

### 工具可用性策略

每个工具声明 `ToolAvailability`：

- **Always** — 始终激活
- **PrimaryLanguage { requires }** — 仅当项目主语言匹配时激活（如 Rust 工具要求 `primary == "rust"`）
- **DetectedLanguage { languages }** — 当项目检测到该语言时激活

用户可通过配置的 `enable` / `disable` / `enabled`（精确模式）覆盖自动检测结果。Plan 模式额外禁用写入和 Git 类工具。

### 工具结果类型

`ToolResult` 是一个带 tag 的枚举，LLM 接收结构化 JSON：

| 类型 | 说明 |
|------|------|
| `FileContent` | 文件内容（含行号和元数据） |
| `Diagnostics` | LSP 诊断列表 |
| `Json` | `cargo check` 的 JSON 诊断（支持 filter） |
| `Matches` | `search_code` 的匹配结果 |
| `WriteConfirmation` | 写入确认（含 diff） |
| `Text` | 通用文本响应（`git_*`、`web_search`、`cargo`、错误消息） |

## 配置

三层配置级联（低 → 高优先级）：

1. 代码默认值（`Config::default()`）
2. `~/.config/bytode.toml`（用户级别）
3. `<项目根>/.bytode.toml`（项目级别，最高优先级）

### 配置示例

```toml
[project]
lang = "rust"

[build]
extra_check_flags = ["--all-targets", "--all-features"]

[tools]
enable = ["web_search"]
disable = ["git_log"]

[tools.search]
max_results = 100
ignore_dirs = ["target", ".git", "node_modules"]

[tools.web_search]
timeout_secs = 10
# proxy = "http://127.0.0.1:7890"

[tools.cargo]
timeout_ms = 180000

[agent]
confirm_before_write = true
max_consecutive_calls = 30
auto_check_after_write = true

[security]
max_file_size = 2097152  # 2 MB
forbidden_write_patterns = ["/etc/*", "/boot/*"]
```

### 安全硬约束

无论配置如何，以下路径**始终**禁止写入：`/etc/*`、`/proc/*`、`/sys/*`。

## 架构

```
src/
├── main.rs           # CLI (clap) — 装配、会话管理、LSP 启动
├── repl.rs           # 异步 REPL — tokio::select! + crossterm EventStream
├── config.rs         # 三层 TOML 配置加载器
├── project.rs        # 标记文件检测（Cargo.toml / go.mod / package.json …）
├── error.rs          # BytodeError 统一错误类型 (thiserror)
├── agent/
│   ├── mod.rs        # Agent — ReAct 循环、模式切换、工具参数格式化
│   ├── context.rs    # ContextBuilder — 核心提示词 + SOP + 上下文估算
│   └── memory.rs     # MemoryLayer — 确定性压缩（>80% 且 >10 轮触发）
├── tools/
│   ├── mod.rs        # Tool trait + ToolRegistry + ToolResult 枚举
│   ├── file.rs       # read_file（含目录列表）/ write_file（原子 + diff）
│   ├── search.rs     # search_code (ripgrep --json)
│   ├── check.rs      # cargo_check (cargo check --json + simple filter)
│   ├── lsp_diag.rs   # get_diagnostics (rust-analyzer 缓存)
│   ├── cargo.rs      # cargo (白名单子命令)
│   ├── web.rs        # web_search (DuckDuckGo HTML 解析)
│   └── git.rs        # git_status / git_diff / git_log
├── lsp/
│   ├── client.rs     # rust-analyzer JSON-RPC 客户端（含 didChange/didOpen）
│   └── types.rs      # LSP 诊断类型
├── llm/
│   └── mod.rs        # DeepSeekClient — SSE 流式 + token 计费 + 会话成本
└── ui/
    ├── mod.rs        # Terminal 封装（ratatui + crossterm）
    ├── layout.rs     # 布局引擎（滚动偏移、侧栏、审批弹窗）
    ├── render.rs     # Markdown 渲染 + Rust 语法高亮 + diff 行着色
    └── statusbar.rs  # 状态栏（模式、耗时、成本、上下文用量）
```

## 核心设计

### ReAct 循环

```
用户输入 → ContextBuilder 构建消息 → LLM 流式响应
  ├── Text → 追加到 UI，返回结果
  └── ToolCall → 执行工具 → 结果注入上下文 → 继续循环
```

- 每次工具调用显示 `⟳ tool_name(args)` 格式的参数摘要
- 连续工具错误超过 5 次自动终止
- `Ctrl+C` 通过 `AtomicBool` 取消令牌中断循环

### 上下文管理

1. **Layer 1 — 核心提示词**：项目快照 + 行为规则 + 可用工具列表（固定，DeepSeek 前缀缓存锚点）
2. **Layer 2 — SOP 提示词**：仅在检测到构建失败时注入，强制要求先用 `get_diagnostics`
3. **Layer 3 — 压缩摘要 + 最近 N 轮**：记忆层管理

Token 估算：`1 token ≈ 4 chars`（序列化 JSON 长度 / 4）。

### 记忆压缩

**确定性压缩**（Rust 代码驱动，非 LLM 摘要）：
- 触发条件：`ctx_used / ctx_total > 0.8` 且 `recent.len() > max_recent`
- 压缩内容：提取任务描述、修改的文件列表、已解决的错误列表
- 新摘要合并到已有摘要末尾

### LLM 集成

- **后端**：DeepSeek API（兼容 OpenAI 格式），通过 `async-openai` 调用
- **流式**：SSE 逐 token 推送，自动解析 `tool_calls` 增量
- **容错**：处理 LLM 拼接多个 JSON 对象的情况，选择键最多的那个
- **计费追踪**：会话累计输入/输出 token + 成本估算
  - 输入：$0.27 / 1M tokens
  - 输出：$1.10 / 1M tokens

### LSP 集成

- 启动 `rust-analyzer` JSON-RPC 子进程
- 维护诊断缓存（`publishDiagnostics` 推送更新）
- `write_file` 成功后发送 `didChange` 通知刷新诊断
- 退出时发送 `shutdown` 请求优雅关闭

### 安全机制

- **零 Shell 访问** — 没有任何 `shell` / `exec` 工具
- **路径沙箱** — 所有读写限制在项目根目录内
- **原子写入** — 先写临时文件再 `rename`，防止写一半崩溃
- **写入确认** — `confirm_before_write` 默认 `true`，TUI 弹出 diff 确认弹窗
- **文件大小限制** — 默认最大 1MB，可配置
- **禁止路径模式** — 硬编码禁止 `/etc/*`、`/proc/*`、`/sys/*`

## 工作流程

```
read_file → 编辑 → cargo_check / get_diagnostics → 重复
```

1. **Read before write** — 编辑前必须先读取文件内容
2. **Diagnose first** — 构建失败后立即调用 `get_diagnostics`，不要 grep 错误日志
3. **Fix one at a time** — 一次修一个错误，修完立即 `cargo_check` 验证
4. **Evidence before claims** — 不说"应该修好了"，以 `cargo_check` filter `"length"` 返回 0 为唯一标准

## 项目检测

通过标记文件自动检测项目语言和构建系统：

| 标记文件 | 语言 | 构建系统 | 内容校验 |
|----------|------|----------|----------|
| `Cargo.toml` | Rust | Cargo | 含 `[package]` 或 `[workspace]` |
| `go.mod` | Go | Go Modules | 以 `module ` 开头 |
| `package.json` | JavaScript | npm | 含 `"name"` |
| `pyproject.toml` | Python | pip | 含 `[project]` 或 `[tool.poetry]` |
| `CMakeLists.txt` | C/C++ | CMake | 无（存在即匹配） |
| `pom.xml` | Java | Maven | 含 `<project` |

多个标记文件共存时按优先级排序（Rust 最高 100，Java 最低 50）。可通过配置 `[project] lang = "rust"` 强制覆盖。

## 会话持久化

会话自动保存到 `~/.bycode/sessions/<hash>.json`：

- 文件名使用项目完整路径的 64-bit SipHash 哈希值（`DefaultHasher`）
- `SessionData` 存储 `project_path`，列表显示时自动将 `$HOME` 替换为 `~`
- 旧版本兼容：`project_path` 缺失时回退到 hash 显示
- 交互模式启动时列出所有可用会话（按修改时间倒序），可选择恢复或新建
- 单次任务模式自动加载已有会话，执行后自动保存
- 保存内容：用户输入、助手回复、工具调用记录（全部序列化为 JSON）
- 过滤策略：保存时剔除仅有错误的轮次

## 日志

日志写入 `/tmp/bytode.log`，使用 `tracing-subscriber`，默认日志级别 `bytode=info`，可通过 `RUST_LOG` 环境变量覆盖。

## License

MIT
