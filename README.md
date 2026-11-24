# litemiyu-acp

参考 [LiteMiyu](https://github.com/miyu-agent/litemiyu) 的视觉美学与 Markdown 渲染引擎，用纯 Rust 打造的超轻量 **ACP v1（Agent Client Protocol）** 终端 TUI 客户端，默认直连 `agy acp`，亦兼容任意标准 ACP v1 Server。

## 特性

- **LiteMiyu 级 Markdown 渲染**：
  - 完整 Box-drawing 表格框线（`┌─┬─┐` / `├─┼─┤` / `└─┴─┘`），每两行间有柔和分隔线，窄屏超宽表自动退化为优雅的「表头：值」竖排卡片；
  - 单元格与正文中英文严格对齐、句读括号避头尾折行、路径智能断行；
  - `[label](file:///...)` 优雅只渲染短链接标题，告别超长绝对路径撑爆表格，点击即可打开本地文件或 URL；
  - 圆角代码块框线 + 内置 20+ 语言词法高亮；
  - 标题层级、引用竖线、强调色列表记号、任务清单、行内/块级数学公式（支持 `\[...\]` / `\(...\)` 自动转换）。
- **Material Design 3 动态主题系统**：内置 5 套配色（`miyu-lavender` / `miyu-night` / `miyu-forest` / `miyu-day` / `terminal`），支持 `/theme` 实时切换与 `/bg` 一键切换透明背景。
- **宽屏双栏与时间线**：
  - 右侧边栏实时展示会话标题、工作区、上下文进度条（`▰▰▱▱`）、Token/缓存命中统计、**跳转消息**、**最近会话**（点击直接切换）；
  - 思考过程与工具调用时间线支持折叠/展开（点击或 `Ctrl+O`），生成时带真彩色流光动画。
- **完整 ACP v1 协议支持**：
  - 动态同步 `/model`、`/effort`、`/mode`（`Shift+Tab`）、`/resume`、全部后端 Skills 斜杠命令补全与 `session/request_permission` 权限审批抽屉。

## 用法

```powershell
# 默认直连 agy acp（在当前目录启动）
litemiyu-acp

# 指定工作目录或恢复历史会话
litemiyu-acp --cwd D:\Code
litemiyu-acp --resume <session-id>

# 离线预览 Markdown 表格、代码块、思考折叠等渲染效果
litemiyu-acp --demo

# 透明背景（继承宿主终端底色）
litemiyu-acp --transparent

# 连接其他 ACP v1 后端（例如带沙盒的 agy acp 或其他 ACP agent）
litemiyu-acp --cmd "agy acp --sandbox"
```

## 常用快捷键与命令

| 按键 / 命令 | 功能 |
| --- | --- |
| `Enter` / `Shift+Enter` (`Ctrl+J`) | 发送消息 / 换行 |
| `Ctrl+V` / `Alt+V` / 拖拽图片 | 粘贴剪贴板截图、复制的图片文件或拖拽图片路径（自动渲染 `[图片 N]` 芯片） |
| `/image [路径]`（别名 `/img`） | 从剪贴板或指定本地路径附加图片 |
| `Shift+Tab` | 快速切换执行模式（`default` / `accept-edits` / `plan`） |
| `Ctrl+B` / `/sidebar` | 显示 / 隐藏右侧边栏 |
| `Ctrl+O` | 展开 / 折叠全部思考过程与工具详情 |
| `PgUp` / `PgDn` / 鼠标滚轮 / `End` | 滚动对话 / 回到最新底部 |
| `Esc` | 中断生成 / 关闭弹窗 / 清空输入 |
| `/model` · `/effort` · `/mode` | 交互式切换模型、思考等级、模式 |
| `/resume` · `/new` | 搜索恢复历史会话 / 新建会话 |
| `/theme` · `/bg` | 切换配色主题 / 切换铺底或透明背景 |
| 鼠标左键拖拽 / `/copy [n\|all]` | 拖拽划词松手自动复制（不串侧边栏） / 复制倒数第 n 条或全部对话 Markdown 原文 |
| `/mouse` | 开关鼠标捕获（关闭后或按住 `Shift` 拖选可直接用宿主终端原生划词） |

## 开源许可

本项目的 Markdown 折行、表格与词法高亮表（`resources/code.json`）及配色思路参考自 [miyu-agent/litemiyu](https://github.com/miyu-agent/litemiyu)，遵循 `AGPL-3.0-or-later` 许可发布。
