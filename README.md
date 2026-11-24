# litemiyu-acp

参考 [LiteMiyu](https://github.com/miyu-agent/litemiyu) 的视觉美学与 Markdown 渲染引擎、并借鉴 [sai](https://github.com/jswysnemc/sai) 交互细节打造的纯 Rust 超轻量 **ACP v1（[Agent Client Protocol](https://agentclientprotocol.com)）** 终端 TUI 客户端。默认直连 `agy acp`，亦可通过 `--cmd` 接入任意基于 `stdio` 的标准 ACP v1 Agent。

## 支持的 ACP 后端（兼容列表）

凡是实现标准 **ACP v1（`protocolVersion: 1`，JSON-RPC 2.0 over stdio）** 的 Agent 后端均可直接接入。只需通过 `--cmd "<启动命令>"` 指定对应后端：

| 后端 / Agent | 接入方式 | 推荐启动命令 | 说明 |
| --- | --- | --- | --- |
| **Antigravity CLI (`agy`)** | 原生 ACP（默认内置） | `litemiyu-acp` 或 `litemiyu-acp --cmd "agy acp --sandbox"` | 默认后端，已深度适配并完整验证（含模型/思考等级/模式/Skills/用量缓存/会话恢复） |
| **Gemini CLI** | 原生 ACP | `litemiyu-acp --cmd "gemini --acp"` | 也可通过 `npx -y @google/gemini-cli --acp` 启动 |
| **OpenCode** | 原生 ACP | `litemiyu-acp --cmd "opencode acp"` | 也可通过 `npx -y opencode-ai acp` 启动 |
| **Claude Code** | 官方 ACP 适配器 | `litemiyu-acp --cmd "npx -y @agentclientprotocol/claude-agent-acp"` | 使用 ACP 官方维护的 `claude-agent-acp` 桥接包 |
| **OpenAI Codex CLI** | 官方 ACP 适配器 | `litemiyu-acp --cmd "npx -y @agentclientprotocol/codex-acp"` | 使用 ACP 官方维护的 `codex-acp` 桥接包 |
| **GitHub Copilot CLI** | 原生 ACP | `litemiyu-acp --cmd "copilot --acp"` | 支持标准 ACP stdio 模式 |
| **Goose** | 原生 ACP | `litemiyu-acp --cmd "goose acp"` | 支持标准 ACP stdio 模式 |
| **Kiro CLI** | 原生 ACP | `litemiyu-acp --cmd "kiro-cli acp"` | 支持标准 ACP stdio 模式 |
| **Cursor CLI** | 原生 ACP | `litemiyu-acp --cmd "agent acp"` | 支持标准 ACP stdio 模式 |
| **其他 ACP v1 Agent** | 任意 stdio ACP Server | `litemiyu-acp --cmd "<binary> [args...]"` | 兼容 [ACP Registry](https://agentclientprotocol.com) 中的其他标准 Agent 或自研后端 |

### 协议能力与使用提示

- **已实现的 ACP v1 能力**：`initialize`、`session/new`、`session/load`、`session/list`、`session/prompt`（文本 + Base64 图片）、`session/cancel`、`session/set_config_option`、`session/set_mode`、`session/request_permission`（权限审批抽屉）、`fs/read_text_file`、`fs/write_text_file` 及全套 `session/update` 流式事件（消息、思考、工具调用/Diff、计划、可用命令、上下文用量）。
- **鉴权复用**：请先在对应 Agent CLI 中完成一次账号登录或 API Key 配置，`litemiyu-acp` 会直接复用该后端的凭据。
- **能力降级**：客户端当前声明 `fs.readTextFile / writeTextFile = true`、`terminal = false`（命令执行交由后端自行管理）；若部分后端未实现 `session/list` / `session/load`，不影响正常对话，仅 `/resume` 会话列表为空。
- **Windows 下使用 `npx`**：PowerShell/CMD 环境下建议写成 `npx.cmd`（例如 `--cmd "npx.cmd -y @agentclientprotocol/claude-agent-acp"`），或全局安装适配器后直接传可执行文件名。

## 特性

- **LiteMiyu × sai 风格终端渲染**：
  - 完整 Box-drawing 表格框线（`┌─┬─┐` / `├─┼─┤` / `└─┴─┘`），窄屏超宽表自动退化为「表头：值」竖排卡片；
  - 中英文严格对齐、句读括号避头尾折行、路径智能断行；`[label](file:///...)` 仅渲染短标题且支持点击打开本地文件/URL；
  - 无边框底纹代码块 + 20+ 语言词法高亮 + Shell 命令语义高亮；划词复制自动剔除语言标签与装饰缩进、无缝还原软折行；
  - 标题层级、引用竖线、列表记号、任务清单、行内/块级数学公式（支持 `\[...\]` / `\(...\)` 自动转换）；
  - 文件编辑 Diff 与文件读取预览默认折叠（展示 `+N -M` / 行号范围），点击展开为带行内词级高亮的统一视图。
- **Material Design 3 动态主题系统**：内置 5 套配色（`miyu-lavender` / `miyu-night` / `miyu-forest` / `miyu-day` / `terminal`），支持 `/theme` 实时切换与 `/bg` 切换透明背景。
- **宽屏双栏、底部动效与多行输入**：
  - 右侧边栏展示会话标题、工作区、上下文进度条（`▰▰▱▱`）、Token/缓存命中统计、**跳转消息**、**最近会话**；底栏右下角常驻当前工作目录；
  - 底部状态行配备等宽眨眼伴随表情与 2 秒余弦扫描流光动效，实时反映思考/读取/写入/执行/生成阶段；
  - 输入框支持随内容自动增高、Windows 无括号多行粘贴防误发、长文本与图片原子芯片删除，以及双行卡片式 `/resume` 会话选择器（支持 `Tab` 切换当前目录 / 全部会话）。

## 安装与运行

```powershell
# 从源码构建并安装到 PATH
cargo install --path .

# 默认直连 agy acp（在当前目录启动）
litemiyu-acp

# 指定工作目录或恢复历史会话
litemiyu-acp --cwd D:\Code
litemiyu-acp --resume <session-id>

# 连接其他 ACP v1 后端
litemiyu-acp --cmd "gemini --acp"
litemiyu-acp --cmd "opencode acp"
litemiyu-acp --cmd "npx.cmd -y @agentclientprotocol/claude-agent-acp"

# 离线预览 Markdown 表格、代码块、Diff、读取预览与思考折叠效果
litemiyu-acp --demo

# 透明背景（继承宿主终端底色）
litemiyu-acp --transparent
```

## 常用快捷键与命令

| 按键 / 命令 | 功能 |
| --- | --- |
| `Enter` / `Shift+Enter` (`Ctrl+J`) | 发送消息 / 换行（多行输入支持 `↑`/`↓` 优先在行间移动光标） |
| `Ctrl+V` / `Alt+V` / 拖拽图片 | 粘贴剪贴板截图、复制的图片文件或拖拽图片路径（以原子芯片渲染，一键整体删除） |
| `/image [路径]`（别名 `/img`） | 从剪贴板或指定本地路径附加图片 |
| `Shift+Tab` | 快速切换执行模式（`default` / `accept-edits` / `plan`） |
| `Ctrl+B` / `/sidebar` | 显示 / 隐藏右侧边栏 |
| `Ctrl+O` | 展开 / 折叠全部思考过程与工具详情（单个读取/编辑卡片亦支持鼠标点击展开） |
| `PgUp` / `PgDn` / 鼠标滚轮 / `End` | 滚动对话 / 回到最新底部 |
| `Esc` | 中断生成 / 关闭弹窗 / 清空输入 |
| `/model` · `/effort` · `/mode` | 交互式切换模型、思考等级、模式 |
| `/resume` · `/new` | 搜索恢复历史会话（`Tab` 切当前目录/全部会话）/ 新建会话 |
| `/theme` · `/bg` | 切换配色主题 / 切换铺底或透明背景 |
| 鼠标左键拖拽 / `/copy [n\|all]` | 拖拽划词松手自动复制（过滤代码块装饰、不串侧边栏）/ 复制倒数第 n 条或全部对话 Markdown 原文 |
| `/mouse` | 开关鼠标捕获（关闭后或按住 `Shift` 拖选可直接用宿主终端原生划词） |

## 开源许可

本项目的 Markdown 折行、表格与词法高亮表（`resources/code.json`）及配色思路参考自 [miyu-agent/litemiyu](https://github.com/miyu-agent/litemiyu)，遵循 `AGPL-3.0-or-later` 许可发布。
