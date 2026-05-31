# bytode

> **by**tode = **编**译 **To**ken **de** 代码 —— 终端 Rust 编程助手

bytode 是一个终端编码代理（ReAct 循环 + DeepSeek + LSP + TUI），专注于 Rust 项目开发。工作在终端内部，通过类型化工具与代码库交互，零 Shell 访问。

## 安装

```bash
git clone <repo>
cd bytode
cargo build --release
```

### 依赖

| 依赖 | 用途 |
|------|------|
| `rust-analyzer` | LSP 诊断 (`get_diagnostics`) |
| `ripgrep` | 代码搜索 (`search_code`) |
| `git` | 版本控制工具 (`git_status` / `git_diff` / `git_log`) |
| Rust 2024 toolchain | 必需 |

### 环境变量

```bash
export DEEPSEEK_API_KEY="sk-..."
```

## 用法

### 交互模式

```bash
cargo run
```

启动 TUI 终端，进入 REPL 循环。支持：
- 选择 / 恢复历史会话
- 实时流式输出，LLM 响应期间键盘仍可操作（滚动 / Ctrl+C / 输入缓冲）
- 工具调用显示参数：`⟳ read_file(src/main.rs, offset=10)` 而非空括号
- 状态栏显示：耗时（ms）、会话成本、上下文用量、当前模式
- 崩溃自动恢复终端（panic 时调用 `Terminal::restore()`）
- Markdown 渲染（粗体、斜体、代码块带 Rust 语法高亮、标题、引用）

### 单次任务

```bash
cargo run -- "在 src/main.rs 中添加一个问候函数"
```

### 指定模型

```bash
cargo run -- --model deepseek-v4-flash
```

### 命令行参数

| 参数 | 简写 | 默认值 | 说明 |
|------|------|--------|------|
| `[TASK]` | — | — | 单次任务内容，省略则进入交互模式 |
| `--project` | `-p` | `.` | 项目根目录 |
| `--api-key` | — | `$DEEPSEEK_API_KEY` | DeepSeek API 密钥 |
| `--model` | — | `deepseek-v4-pro` | 模型名称 |

## REPL 命令

| 命令 | 说明 |
|------|------|
| `/exit` / `/quit` | 退出 |
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
| `Ctrl+C` | 退出 / 取消当前 LLM 调用 |
| `Ctrl+T` | 切换工具侧栏 |
| `↑` / `PageUp` | 向上滚动历史（更旧，步长 3 / 20 行） |
| `↓` / `PageDown` | 向下滚动历史（更新，步长 3 / 20 行） |

## 模式

### Build 模式（默认）

完整工具集，可读写文件、运行构建、操作 Git。

### Plan 模式（只读）

禁用以下工具：
- `write_file`
- `git_status` / `git_diff` / `git_log`

其他工具（`read_file`、`search_code`、`get_diagnostics`、`run_check`、`web_search`）照常可用。切换时即时重建核心提示词中的工具列表。

## 工具列表

| 工具 | 类型 | 可用性 | 说明 |
|------|------|--------|------|
| `read_file` | 只读 | 始终 | 读取文件或列出目录。文件返回行号内容，目录返回 `ls` 风格列表（含文件大小） |
| `write_file` | 修改 | 始终 | 原子写入（临时文件 + 重命名），带 diff 确认，写入后自动通知 LSP 刷新诊断 |
| `search_code` | 只读 | 始终 | 使用 ripgrep 搜索代码库，返回匹配项，结果带预览 |
| `run_check` | 构建 | Rust | 运行 `cargo check --message-format json` |
| `get_diagnostics` | 只读 | Rust | 从 rust-analyzer 读取缓存的诊断信息，支持 path/filter 参数 |
| `run_cargo` | 构建 | Rust | 运行 Cargo 子命令（白名单：check/build/test/clippy/fmt/doc 等） |
| `web_search` | 只读 | 始终 | DuckDuckGo 网页搜索（无需 API Key），可配置代理和超时 |
| `git_status` | 只读 | 始终 | `git status --porcelain` |
| `git_diff` | 只读 | 始终 | `git diff` / `git diff --cached`，支持 staged 和 path 参数 |
| `git_log` | 只读 | 始终 | `git log --oneline`，可配置 count 和 path |

## 配置

支持三层配置级联（低 → 高优先级）：

1. 代码默认值
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

## 架构

```
src/
├── main.rs           # CLI (clap)，装配和初始化
├── repl.rs           # 异步 REPL 循环（tokio::select! + EventStream）
├── config.rs         # TOML 配置，三级加载器
├── project.rs        # 标记检测项目语言/构建系统
├── error.rs          # BytodeError (thiserror)
├── agent/
│   ├── mod.rs        # ReAct 循环，模式管理，工具调用（含参数格式化）
│   ├── context.rs    # 上下文构建器（系统提示词 + SOP）
│   └── memory.rs     # 会话记忆层（压缩、持久化）
├── tools/
│   ├── mod.rs        # Tool trait（含 format_result_for_display）+ ToolRegistry
│   ├── file.rs       # read_file（含目录列表）/ write_file
│   ├── search.rs     # search_code (ripgrep)
│   ├── check.rs      # run_check (cargo check)
│   ├── lsp_diag.rs   # get_diagnostics
│   ├── cargo.rs      # run_cargo
│   ├── web.rs        # web_search (DuckDuckGo)
│   └── git.rs        # git_status / git_diff / git_log
├── lsp/              # rust-analyzer JSON-RPC 客户端（含 didChange 通知）
├── llm/              # DeepSeek 客户端 (async-openai)
└── ui/
    ├── layout.rs     # 布局（块渲染、滚动偏移）
    ├── render.rs     # Markdown 渲染 + Rust 语法高亮 + diff 行着色
    └── statusbar.rs  # 状态栏（模式、耗时、成本、上下文）
```

## 核心设计原则

- **零 Shell 访问**：所有操作通过类型化工具完成
- **异步 REPL**：`tokio::select!` 并发处理键盘事件和 LLM 响应，思考时不阻塞输入
- **构建输出仅 JSON**：`cargo check --message-format json`
- **LSP 唯一诊断来源**：禁止 grep 错误日志；写入文件后自动发送 `didChange` 刷新
- **原子写入**：临时文件 + 重命名，默认需确认
- **确定性内存压缩**：Rust 代码驱动（>80% 上下文占用且 >10 轮时触发）
- **Token 估算**：1 token ≈ 4 chars
- **崩溃恢复**：`catch_unwind` 包裹 REPL，panic 时自动恢复终端
- **DeepSeek 定价**：输入 $0.27/1M tokens，输出 $1.10/1M tokens

## 工作流程

```
read_file → 编辑 → run_check / get_diagnostics → 重复
```

1. **Read before write** — 编辑前必须先读取
2. **Diagnose first** — 构建失败后立即调用 `get_diagnostics`
3. **Fix one at a time** — 一次修一个错误，修完立即 `run_check` 验证
4. **Evidence before claims** — 不报"应该修好了"，只报"诊断为零"

## 会话持久化

会话自动保存到 `~/.bycode/sessions/<sha256>.json`：
- 文件名使用项目完整路径的哈希值，避免碰撞
- SessionData 存储项目路径，列表显示时展示 `~/Document/bytode` 而非 hash
- 旧 session 兼容（`project_path` 缺失时回退到 hash 显示）
- 交互模式启动时列出可用会话，可选择恢复
- 单次任务模式自动加载已有会话，执行后自动保存
- 保存内容：用户输入、助手回复、工具调用记录（序列化为 JSON）

## License

MIT
