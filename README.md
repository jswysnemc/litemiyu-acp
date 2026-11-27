# agy-tui

[![CI](https://github.com/jswysnemc/agy-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/jswysnemc/agy-tui/actions/workflows/ci.yml)
[![Release](https://github.com/jswysnemc/agy-tui/actions/workflows/release.yml/badge.svg)](https://github.com/jswysnemc/agy-tui/releases)

`agy-tui` 是专为 **Antigravity CLI（`agy`）** 打造的纯 Rust 超轻量终端 TUI 前端。项目视觉风格参考 [LiteMiyu](https://github.com/miyu-agent/litemiyu)，交互细节借鉴 [sai](https://github.com/jswysnemc/sai)，默认零配置直连 `agy acp`。

后续功能演进以深度适配 **Agy CLI** 生态能力为主轴；同时底层基于标准 **ACP v1（[Agent Client Protocol](https://agentclientprotocol.com)）** 实现，附带兼容其他基于 `stdio` 的标准 ACP v1 后端。

## 为 Agy CLI 深度定制的能力

- **开箱即用**：直接运行 `agy-tui` 即可拉起 `agy acp`，复用本地 `agy` 的账号鉴权与环境配置，退出时自动清理子进程组。
- **模型、思考等级与模式切换**：内置 `/model`、`/effort`、`/mode` 及 `Shift+Tab` 快捷键，即时切换模型、思考等级和执行模式（`default` / `accept-edits` / `plan`），并在顶栏与侧栏展示账号等级、上下文进度条与 Token 缓存命中统计。
- **任务清单全链路追踪**：直接解析 `agy` 的 `update_steps`（同时兼容标准 ACP `plan`），在对话时间线原地更新，并提供底部常驻停靠条（`Ctrl+T` / `/todo` 展开收起）及宽屏侧栏定位。
- **交互式提问抽屉**：原生支持 `agy ask_question`（ACP `elicitation/create`），在底部抽屉中完成单选、多选与自定义补充输入，不阻塞界面渲染。
- **子代理运行观测**：在本地 `agy` 模式下以只读增量方式追踪子代理状态与 `transcript.jsonl` 摘要，并在侧栏展示后台任务计数与耗时，点击可定位至对应工具行。
- **生成期消息排队与即时插话**：支持通过 `/queue` 切换「排队等待（`queue`）」与「即时插话（`steer`）」，或在生成中按 `Ctrl+S` 立即并发插话，空输入框按 `↑` 可撤回待发消息继续编辑。
- **跨目录会话管理与 Skills 补全**：`/resume` 支持按「当前目录 / 全部会话」检索并恢复历史会话（`Tab` 切换范围）；输入框内置 `/`（命令）、`#`（`SKILL.md` 模糊搜索并自动转译引用）、`@`（工作区文件）三触发符悬浮补全。

## 终端渲染与交互特性

- **高质量 Markdown 与代码呈现**：
  - 完整 Box-drawing 表格框线（`┌─┬─┐` / `├─┼─┤` / `└─┴─┘`），窄屏超宽表自动退化为竖排卡片；
  - 轻量 Unicode 框线渲染 Mermaid 图表（支持 `flowchart`、`sequenceDiagram`、`stateDiagram`，窄屏自动回退竖排布局）；
  - 中英文混排严格对齐、句读括号避头尾折行、路径分层高亮与智能断行；`[label](file:///...)` 仅展示短标题，点击可直接打开本地文件或 URL；
  - 无边框底纹代码块，内置 20+ 语言词法高亮与 Shell 命令语义高亮；划词复制自动剔除语言标签与装饰缩进并还原软折行；
  - 支持标题层级、引用竖线、列表与任务清单、以及 `\[...\]` / `\(...\)` 数学公式自动转换；
  - 文件读取预览与编辑 Diff 默认折叠（展示行号范围或 `+N -M` 变更统计），点击可展开为带行内词级高亮的统一视图。
- **响应式界面与主题系统**：
  - 居中自适应启动首页，内置可点击配置项与当前目录最近会话直达列表；
  - 宽屏双栏布局，对话滚动时顶部固定展示黏性用户消息条（点击回跳），底部配备眨眼状态指示与流光动效；
  - 内置 14 套深浅色配色主题，支持 `/theme` 色板预览、上下实时切换与自定义 JSON 配色，以及 `/bg` 透明背景切换。
- **流畅的多平台输入与剪贴板体验**：
  - 输入框随内容自动增高，支持 `Shift+Enter` 换行（Unix 启用 Kitty 键盘增强协议）、`$VISUAL` / `$EDITOR` 外部编辑器（`Ctrl+G`）、Windows 无括号多行粘贴防误发、长文本与图片原子芯片一键删除；
  - 支持 Windows 及 Linux（Wayland `wl-paste` / X11 `xclip`）剪贴板截图粘贴与拖拽图片，截图缓存自动滚动保留最近 50 份；
  - 左键拖拽划词松手自动复制（过滤代码块装饰、不跨侧边栏），Unix 启用电平触发 `use-dev-tty` 轮询与 DEC Mode 2026 同步帧刷新，降低高频按键与滚轮卡顿。

## 安装与运行

可直接从 [GitHub Releases](https://github.com/jswysnemc/agy-tui/releases) 下载对应平台预编译包（Windows x64 / Linux x64 & arm64 / macOS arm64 & x64，附 `SHA256SUMS.txt`），或从源码构建：

```bash
# 从源码构建并安装到 PATH
cargo install --path .

# 默认直连 agy acp（在当前目录启动）
agy-tui

# 透传 agy 启动参数（如沙箱模式）
agy-tui --cmd "agy acp --sandbox"

# 指定工作目录或恢复历史会话
agy-tui --cwd /path/to/workspace
agy-tui --resume <session-id>

# 指定配色主题（支持内置 ID/别名或自定义主题 ID）或启用透明背景
agy-tui --theme catppuccin-mocha
agy-tui --transparent

# 离线预览表格、代码块、Diff、读取预览与思考折叠效果
agy-tui --demo
```

配置与缓存默认独立存放于 `$XDG_CONFIG_HOME/agy-tui`（Windows 为 `%APPDATA%\agy-tui`）及 `$XDG_CACHE_HOME/agy-tui`（Windows 为 `%LOCALAPPDATA%\agy-tui`）。从旧版 `litemiyu-acp` 升级时，首次启动会自动读取并迁移原有偏好配置。

## 常用快捷键与命令

| 按键 / 命令 | 功能 |
| --- | --- |
| `Enter` / `Shift+Enter` (`Ctrl+J`) | 发送消息 / 输入换行（多行输入支持 `↑`/`↓` 在行间移动光标） |
| `Ctrl+S` / `↑`（空输入框） | 生成中立即并发插话（或发出队首） / 撤回排队消息至输入框 |
| `/queue [queue\|steer]` | 切换排队等待或即时插话策略，或打开待发队列管理 |
| `Ctrl+A` / `Ctrl+E` | 光标移至输入框最开头 / 最末尾（`Home`/`End` 移至当前行首尾） |
| `Ctrl+G`（别名 `Alt+G`） | 调用 `$VISUAL` / `$EDITOR`（Windows 默认 `notepad.exe`）编辑正文，保存后自动写回 |
| `/` · `#` · `@` | 触发命令、Skills、工作区文件悬浮补全（`↑↓` 选择，`Tab` 补全，`Enter` 插入或执行） |
| `Ctrl+V` / `Alt+V` / 拖拽图片 | 粘贴剪贴板截图、图片文件或拖拽路径（原子芯片一键删除） |
| `/image [路径]`（别名 `/img`） | 从剪贴板或指定本地路径附加图片 |
| `Shift+Tab` | 循环切换执行模式（`default` / `accept-edits` / `plan`） |
| `Ctrl+B` / `/sidebar` | 显示 / 隐藏右侧边栏 |
| `Ctrl+T` / `/todo` | 展开 / 收起底部任务清单停靠条 |
| `Ctrl+O` | 展开 / 折叠全部思考过程与工具详情（单个读取/编辑卡片支持鼠标点击展开） |
| `PgUp` / `PgDn` / 滚轮 / `End` | 滚动对话 / 回到底部并恢复自动跟随 |
| `Esc` | 中断当前生成 / 关闭浮层 / 清空输入 |
| `/model` · `/effort` · `/mode` | 交互式切换模型、思考等级、执行模式 |
| `/resume` · `/new` · `/clear` | 检索恢复历史会话（`Tab` 切换当前目录与全部会话） / 开启新会话 |
| `/theme [id\|reload\|export\|dir]` · `/bg` | 预览切换内置/自定义主题、热重载或导出配色模板 / 开关透明背景 |
| 鼠标左键拖拽 / `/copy [n\|all]` | 拖拽划词松手自动复制 / 复制倒数第 n 条或全部对话 Markdown 原文 |
| `/mouse` | 开关鼠标捕获（关闭后或按住 `Shift` 可使用宿主终端原生划词） |
| `/help` · `/quit` | 查看内置快捷键帮助 / 退出 `agy-tui` |

## 主题系统与自定义配色

`agy-tui` 内置 14 套主题，打开 `/theme` 即可通过右侧 5 色微型色板与界面实时预览切换（`Esc` 自动还原，`Enter` 确认持久化）：

- **深色主题（10 套）**：`miyu-lavender`（默认莫兰迪紫蓝）、`miyu-night`（Tokyo Night）、`catppuccin-mocha`（柔绒暖紫黑）、`rose-pine`（月光玫瑰松）、`kanagawa-wave`（浮世绘墨浪）、`nord`（北欧极夜蓝灰）、`everforest-dark`（森雾暖暗绿）、`gruvbox-dark`（复古胶片暖棕）、`dracula`（霓虹魅紫）、`miyu-forest`（MD3 鼠尾草暗色）。
- **浅色主题（3 套）**：`catppuccin-latte`（拿铁柔白）、`miyu-day`（冷调清浅）、`miyu-paper`（暖调纸质浅色）。
- **透明主题（1 套）**：`terminal`（透明继承宿主终端背景）。

### 自定义配色（JSON）

将自定义主题放入 `$XDG_CONFIG_HOME/agy-tui/themes/*.json`（Windows 为 `%APPDATA%\agy-tui\themes\*.json`），打开 `/theme` 或执行 `/theme reload` 即可热加载生效。支持三种定制方式：

1. **导出模板直接修改**：在对话中输入 `/theme export my-ocean`（或 `/theme init`），程序会将当前主题的完整 44 色导出为可编辑 JSON，并自动复制文件路径；`/theme dir` 可查看并复制主题目录。
2. **单色种子自动派生**：只需指定 `base` 与 `seed`，引擎会自动通过 Material Design 3 HCT 算法派生整套和谐调色板。
3. **局部或全量覆写**：在 `colors` 中按需覆写任意颜色槽（支持 `#RRGGBB`、`#RGB`、`transparent`/`reset`，兼容 `snake_case` 与 `kebab-case` 字段名），未填写的槽位自动继承 `base`。

示例（`~/.config/agy-tui/themes/ocean.json`）：

```json
{
  "name": "Deep Ocean (自定义)",
  "base": "catppuccin-mocha",
  "seed": "#38bdf8",
  "transparent": false,
  "colors": {
    "background": "#0f172a",
    "accent": "#38bdf8",
    "picked": "#c084fc",
    "code_keyword": "#f472b6"
  }
}
```

## 附带兼容其他 ACP v1 后端

除默认深度适配的 `agy` 外，凡实现标准 **ACP v1（`protocolVersion: 1`，JSON-RPC 2.0 over stdio）** 的 Agent 均可通过 `--cmd` 附带接入：

| 后端 / Agent | 接入方式 | 启动命令示例 |
| --- | --- | --- |
| **Gemini CLI** | 原生 ACP | `agy-tui --cmd "gemini --acp"` |
| **OpenCode** | 原生 ACP | `agy-tui --cmd "opencode acp"` |
| **Claude Code** | 官方 ACP 适配器 | `agy-tui --cmd "npx -y @agentclientprotocol/claude-agent-acp"` |
| **OpenAI Codex CLI** | 官方 ACP 适配器 | `agy-tui --cmd "npx -y @agentclientprotocol/codex-acp"` |
| **GitHub Copilot CLI** | 原生 ACP | `agy-tui --cmd "copilot --acp"` |
| **Goose** | 原生 ACP | `agy-tui --cmd "goose acp"` |
| **Kiro CLI** | 原生 ACP | `agy-tui --cmd "kiro-cli acp"` |
| **Cursor CLI** | 原生 ACP | `agy-tui --cmd "agent acp"` |
| **其他 ACP v1 Agent** | 任意 stdio ACP Server | `agy-tui --cmd "<binary> [args...]"` |

兼容说明：

- **鉴权复用**：先在对应 Agent CLI 完成登录或 API Key 配置，`agy-tui` 启动后直接复用该后端的本地凭据。
- **能力范围**：客户端已实现 `initialize`、`session/new`、`session/load`、`session/list`、`session/prompt`（文本与 Base64 图片）、`session/cancel`、`session/set_config_option`、`session/set_mode`、`session/request_permission`、`elicitation/create`、`fs/read_text_file`、`fs/write_text_file` 及全套 `session/update` 流式事件；其中 `terminal = false`（命令执行交由后端自行管理）。若第三方后端未实现部分可选能力（如 `session/list` 或 `agy` 本地子代理日志），界面会自动降级，不影响基础对话。
- **Windows 下使用 `npx`**：PowerShell/CMD 环境建议写为 `npx.cmd`（例如 `--cmd "npx.cmd -y @agentclientprotocol/claude-agent-acp"`），或全局安装适配器后直接传入可执行文件名。

## 开源许可

本项目的 Markdown 折行、表格与词法高亮表（`resources/code.json`）及配色思路参考自 [miyu-agent/litemiyu](https://github.com/miyu-agent/litemiyu)，遵循 `AGPL-3.0-or-later` 许可发布。

## 友情链接

- [LINUX DO](https://linux.do/) — 新的理想型社区
