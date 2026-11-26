//! 应用核心状态机：整合 ACP 异步事件、键盘/鼠标交互、斜杠命令与用户偏好持久化。

mod commands;
mod elicitation;
mod events;
mod keyboard;
mod mouse;
mod picker;
mod queue;

pub use elicitation::ElicitationState;
pub use queue::{InFlightPrompt, QueuedMessage, QueuedMode};

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::acp::{
    AcpClient, ConfigOption, ModeInfo, PermissionRequest, SessionSummary, SlashCommandInfo,
    ToolDiff,
};
use crate::background::BackgroundTracker;
use crate::clipboard::LoadedImage;
use crate::completion::{CompletionCache, CompletionMenu};
use crate::input::InputEditor;
use crate::paste::WindowsPasteState;
use crate::theme::{self, ThemePreset};
use crate::transcript::Transcript;
use crate::ui::overlay::PickerState;
use crate::ui::rows::MdCache;
use crate::ui::{FrameHits, HoverTarget, TextSelection, ViewState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPrefs {
    #[serde(default = "default_theme_id")]
    pub theme: String,
    #[serde(default = "default_true")]
    pub paint_bg: bool,
    #[serde(default = "default_true")]
    pub show_sidebar: bool,
    #[serde(default)]
    pub queued_mode: QueuedMode,
}

fn default_theme_id() -> String {
    ThemePreset::MiyuLavender.id().to_string()
}

fn default_true() -> bool {
    true
}

impl Default for UserPrefs {
    fn default() -> Self {
        Self {
            theme: default_theme_id(),
            paint_bg: true,
            show_sidebar: true,
            queued_mode: QueuedMode::default(),
        }
    }
}

impl UserPrefs {
    pub fn load() -> Self {
        // 1. 优先读取独立配置路径（$XDG_CONFIG_HOME/litemiyu-acp/prefs.json 或 %APPDATA%\litemiyu-acp\prefs.json）
        if let Some(p) = crate::paths::prefs_path()
            && let Ok(s) = std::fs::read_to_string(&p)
            && let Ok(prefs) = serde_json::from_str::<Self>(&s)
        {
            return prefs;
        }
        // 2. 兼容读取旧版 ~/.gemini/litemiyu-acp.json，并自动落盘迁移到独立配置路径
        if let Some(legacy) = crate::paths::legacy_prefs_path()
            && let Ok(s) = std::fs::read_to_string(&legacy)
            && let Ok(prefs) = serde_json::from_str::<Self>(&s)
        {
            prefs.save();
            return prefs;
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Some(p) = crate::paths::prefs_path() {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, json);
            }
        }
    }
}

const LOCAL_COMMANDS: &[(&str, &str)] = &[
    ("model", "切换模型"),
    ("effort", "切换思考等级 (low/medium/high/xhigh)"),
    ("mode", "切换执行模式 (default/accept-edits/plan)"),
    ("queue", "切换排队等待 / 即时插话模式或管理待发队列"),
    ("resume", "浏览并恢复历史会话"),
    ("new", "开启新会话"),
    ("clear", "清空并开启新会话"),
    ("image", "粘贴剪贴板图片或附加本地图片路径 (/image [路径])"),
    ("theme", "切换配色主题"),
    ("bg", "切换铺底色 / 透明继承终端背景"),
    ("sidebar", "显示 / 隐藏右侧边栏 (Ctrl+B)"),
    ("copy", "复制回答原文（/copy [n|all]）；或鼠标直接拖拽划词自动复制"),
    ("mouse", "开关鼠标捕获（关闭后可直接终端划词）"),
    ("help", "查看快捷键与命令帮助"),
    ("quit", "退出 litemiyu-acp"),
];

#[derive(Debug, Clone)]
pub struct PendingImage {
    pub label: String,
    pub image: LoadedImage,
}

pub struct App {
    pub client: Option<AcpClient>,
    pub transcript: Transcript,
    pub editor: InputEditor,
    pub cwd: String,
    pub agent_banner: String,
    pub config_options: Vec<ConfigOption>,
    pub modes: Vec<ModeInfo>,
    pub current_mode: String,
    pub fallback_model_name: String,
    pub tier: Option<String>,
    pub available_commands: Vec<SlashCommandInfo>,
    pub recent_sessions: Vec<SessionSummary>,
    pub pending_resume: Option<String>,
    pub background: BackgroundTracker,
    pub subagents: crate::subagent::Tracker,
    pub busy: bool,
    pub queued_mode: QueuedMode,
    pub queued_messages: VecDeque<QueuedMessage>,
    pub in_flight_prompts: Vec<InFlightPrompt>,
    pub next_prompt_id: u64,
    pub async_turn_last_event: Option<Instant>,
    pub async_turn_saw_usage: bool,
    pub connecting: bool,
    pub scroll: usize,
    pub auto_follow: bool,
    pub show_sidebar: bool,
    pub mouse_capture: bool,
    pub mouse_toggle_pending: Option<bool>,
    pub external_editor_pending: bool,
    pub all_expanded: bool,
    pub tick: usize,
    pub started_at: Instant,
    pub toast: Option<(String, bool, Instant)>,
    pub completion_cache: CompletionCache,
    pub completion_selected: usize,
    pub completion_dismissed_text: Option<String>,
    pub pending_images: Vec<PendingImage>,
    pub image_seq: usize,
    pub windows_paste: WindowsPasteState,
    pub selection: Option<TextSelection>,
    pub hover: HoverTarget,
    pub dragging: bool,
    pub dragging_scrollbar: bool,
    pub picker: Option<PickerState>,
    pub permission: Option<(PermissionRequest, usize)>,
    pub elicitation: Option<ElicitationState>,
    pub last_ctrl_c: Option<Instant>,
    pub should_quit: bool,
    pub md_cache: MdCache,
    pub last_hits: FrameHits,
}

impl App {
    pub fn new(
        client: Option<AcpClient>,
        cwd: String,
        pending_resume: Option<String>,
        show_sidebar: bool,
        demo: bool,
    ) -> Self {
        let mut app = Self {
            client,
            transcript: Transcript::new(),
            editor: InputEditor::new(),
            cwd,
            agent_banner: if demo {
                "Demo 预览模式（内置样例）".into()
            } else {
                "等待握手…".into()
            },
            config_options: Vec::new(),
            modes: Vec::new(),
            current_mode: "default".into(),
            fallback_model_name: if demo {
                "Gemini 4 Neon".into()
            } else {
                "连接中…".into()
            },
            tier: None,
            available_commands: Vec::new(),
            recent_sessions: Vec::new(),
            pending_resume,
            background: BackgroundTracker::default(),
            subagents: Default::default(),
            busy: false,
            queued_mode: QueuedMode::default(),
            queued_messages: VecDeque::new(),
            in_flight_prompts: Vec::new(),
            next_prompt_id: 1,
            async_turn_last_event: None,
            async_turn_saw_usage: false,
            connecting: !demo,
            scroll: 0,
            auto_follow: true,
            show_sidebar,
            mouse_capture: true,
            mouse_toggle_pending: None,
            external_editor_pending: false,
            all_expanded: false,
            tick: 0,
            started_at: Instant::now(),
            toast: None,
            completion_cache: CompletionCache::default(),
            completion_selected: 0,
            completion_dismissed_text: None,
            pending_images: Vec::new(),
            image_seq: 0,
            windows_paste: WindowsPasteState::default(),
            selection: None,
            hover: HoverTarget::None,
            dragging: false,
            dragging_scrollbar: false,
            picker: None,
            permission: None,
            elicitation: None,
            last_ctrl_c: None,
            should_quit: false,
            md_cache: MdCache::new(),
            last_hits: FrameHits::default(),
        };
        if demo {
            app.load_demo_content();
        }
        app
    }

    pub fn load_demo_content(&mut self) {
        self.transcript.session_id = Some("demo-8a9f2c11".into());
        self.transcript.title = Some("无限工作台重启锁屏无锁图标分析".into());
        self.transcript.context_used = 18_400;
        self.transcript.context_window = 1_048_576;
        self.transcript.total_input = 16_000;
        self.transcript.total_output = 2_400;
        self.transcript.total_cached_read = 11_200;
        self.tier = Some("Google AI Ultra".into());

        let now = chrono::Local::now();
        self.recent_sessions = vec![
            SessionSummary {
                session_id: "demo-8a9f2c11".into(),
                cwd: self.cwd.clone(),
                title: "无限工作台重启锁屏无锁图标分析".into(),
                updated_at: (now - chrono::Duration::minutes(3)).to_rfc3339(),
            },
            SessionSummary {
                session_id: "demo-3c71b402".into(),
                cwd: self.cwd.clone(),
                title: "优化 Windows 多行粘贴与原子芯片删除".into(),
                updated_at: (now - chrono::Duration::hours(2)).to_rfc3339(),
            },
            SessionSummary {
                session_id: "demo-9f02a718".into(),
                cwd: r"D:\Code\Systemui-dev".into(),
                title: "Android SystemUI 锁屏过渡动画排查".into(),
                updated_at: (now - chrono::Duration::days(1)).to_rfc3339(),
            },
        ];

        self.transcript.push_user(
            "请给出无限工作台重启锁屏无锁图标的根因修复方案汇总。".into(),
            Vec::new(),
        );
        self.transcript.append_thought(
            "检查 OffToLockscreenTransitionViewModel 与 DeviceEntryIconViewModel：\n\
             1. OffToLockscreenTransitionViewModel 缺少 onFinish = { 1f }，导致冷订阅晚于 FINISHED 时停留在 0f。\n\
             2. DeviceEntryIconViewModel.transitionAlpha 未订阅 isFinishedIn(LOCKSCREEN)。\n\
             汇总成清晰方案并给出验证方式。",
        );
        self.transcript.finish_open_thoughts();
        self.transcript.upsert_tool(
            "call_demo_read".into(),
            Some("read_file".into()),
            Some(r"D:\Code\Systemui-dev\src\com\android\systemui\keyguard\ui\viewmodel\OffToLockscreenTransitionViewModel.kt".into()),
            Some("read".into()),
            Some("completed".into()),
            Some(serde_json::json!({
                "Path": "D:/Code/Systemui-dev/src/com/android/systemui/keyguard/ui/viewmodel/OffToLockscreenTransitionViewModel.kt",
                "StartLine": 44,
                "EndLine": 49
            })),
            Some("44: val lockscreenAlpha: Flow<Float> =\n45:     transitionAnimation.sharedFlow(\n46:         duration = 250.milliseconds,\n47:         onStep = { it },\n48:     )\n".into()),
            Vec::new(),
        );
        self.transcript.upsert_tool(
            "call_demo_cmd".into(),
            Some("run_command".into()),
            Some("cargo test --release && git status -sb".into()),
            Some("execute".into()),
            Some("completed".into()),
            Some(serde_json::json!({ "CommandLine": "cargo test --release && git status -sb" })),
            Some("test result: ok. 17 passed; 0 failed".into()),
            Vec::new(),
        );
        self.transcript.upsert_tool(
            "call_demo_1".into(),
            Some("edit_file".into()),
            Some("OffToLockscreenTransitionViewModel.kt".into()),
            Some("edit".into()),
            Some("completed".into()),
            Some(serde_json::json!({ "Path": "D:/Code/Systemui-dev/src/com/android/systemui/keyguard/ui/viewmodel/OffToLockscreenTransitionViewModel.kt" })),
            None,
            vec![ToolDiff {
                path: r"D:\Code\Systemui-dev\src\com\android\systemui\keyguard\ui\viewmodel\OffToLockscreenTransitionViewModel.kt".into(),
                old_text: "val lockscreenAlpha: Flow<Float> =\n    transitionAnimation.sharedFlow(\n        duration = 250.milliseconds,\n        onStep = { it },\n    )\n".into(),
                new_text: "val lockscreenAlpha: Flow<Float> =\n    transitionAnimation.sharedFlow(\n        duration = 250.milliseconds,\n        onStep = { it },\n        onFinish = { 1f },\n    )\n".into(),
            }],
        );
        self.transcript.append_assistant(
            "## 修复方案输出\n\n\
             | 字段 | 内容 |\n\
             | --- | --- |\n\
             | **方案属性** | 切中根因方案（Root-Cause Fix）— 从源头补齐 `OFF -> LOCKSCREEN` 终态输出并消除 `DeviceEntryIconViewModel.transitionAlpha` 懒加载冷订阅与 `finishedKeyguardState` 的异步竞态窗口。 |\n\
             | **关键调用链** | `KeyguardViewMediator.handleBootInteractor()` → `KeyguardTransitionBootInteractor.start()` → `OffToLockscreenTransitionViewModel.deviceEntryParentViewAlpha` → `DeviceEntryIconViewBinder.bind()` |\n\
             | **同语义对齐** | 已对齐：[KeyguardRootViewModel.kt:186-219](file:///D:/Code/Systemui-dev/KeyguardRootViewModel.kt)、[KeyguardQuickAffordancesCombinedViewModel.kt:89-111](file:///D:/Code/Systemui-dev/KeyguardQuickAffordancesCombinedViewModel.kt) |\n\
             | **改动点** | 1. [OffToLockscreenTransitionViewModel.kt:45-51](file:///D:/Code/Systemui-dev/OffToLockscreenTransitionViewModel.kt) 补充 `onFinish = { 1f }`；2. [DeviceEntryIconViewModel.kt:95-100](file:///D:/Code/Systemui-dev/DeviceEntryIconViewModel.kt) 在 `merge` 中合入 `isFinishedIn(KeyguardState.LOCKSCREEN)` |\n\
             | **归属与门控** | **LSSI 共性**，所有 LSSI/Malibu 机型通用；共性问题，**不加门控**。 |\n\n\
             ### 关键改动预览\n\n\
             ```kotlin\n\
             // OffToLockscreenTransitionViewModel.kt\n\
             val lockscreenAlpha: Flow<Float> =\n\
             \x20   transitionAnimation.sharedFlow(\n\
             \x20       duration = 250.milliseconds,\n\
             \x20       onStep = { it },\n\
             \x20       onFinish = { 1f }, // 补齐终态，避免冷订阅漏收\n\
             \x20   )\n\
             ```\n\n\
             > 注：点击上方 `◇ 思考` 或 `✓ 读取` 可原地展开细节；输入 `/theme` 可实时切换 6 套配色主题。",
        );
        self.transcript
            .finish_turn("Gemini 4 Neon · High", "end_turn", None);
    }

    pub fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
    }

    /// 判断当前会话是否存在正在运行的后台任务（供主循环驱动轻量刷新节拍）。
    ///
    /// 返回:
    /// - 有后台任务运行时返回 `true`
    pub fn has_background_tasks(&self) -> bool {
        self.background.has_running() || (!self.connecting && self.subagents.watching())
    }

    /// 判断当前是否处于生成动效活跃态（包含前台回合与后台任务唤醒后的异步续答）。
    ///
    /// 返回:
    /// - 需要显示底行流光状态行并按 32ms 节拍刷新时返回 `true`
    pub fn is_generating(&self) -> bool {
        self.busy || self.async_turn_last_event.is_some()
    }

    /// 返回下一次应用内定时事件（Toast 过期或异步续答静默收尾）的剩余等待时长。
    ///
    /// 返回:
    /// - 有待触发定时器时返回 `Some(Duration)`，完全无定时需求时返回 `None`
    pub fn next_timer_delay(&self) -> Option<Duration> {
        let mut min_delay: Option<Duration> = None;
        let mut consider = |d: Duration| {
            min_delay = Some(match min_delay {
                Some(cur) => cur.min(d),
                None => d,
            });
        };
        if let Some((_, _, at)) = &self.toast {
            let rem = Duration::from_secs(3).saturating_sub(at.elapsed());
            consider(rem);
        }
        if let Some(last_ev) = self.async_turn_last_event
            && !self.busy
            && !self.background.has_foreground_tool()
        {
            let limit = if self.async_turn_saw_usage {
                Duration::from_millis(500)
            } else {
                Duration::from_millis(1500)
            };
            consider(limit.saturating_sub(last_ev.elapsed()));
        }
        min_delay
    }

    /// 推进应用内定时状态：清理已过期的 Toast，以及在异步续答静默结束后自动补全轮次结束行。
    ///
    /// 返回:
    /// - 状态发生变化（需要重绘一帧）时返回 `true`
    pub fn tick_timers(&mut self) -> bool {
        let mut changed = !self.connecting && self.subagents.poll(self.transcript.session_id.as_deref());
        // 1. 清理超时 3 秒的 Toast，确保清除后空闲循环零空转
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() >= Duration::from_secs(3))
        {
            self.toast = None;
            changed = true;
        }
        // 2. 服务端异步续答无 PromptFinished，静默超时且无前台工具时自动收尾补全 ▣ 结束行
        if let Some(last_ev) = self.async_turn_last_event
            && !self.busy
            && !self.background.has_foreground_tool()
        {
            let idle_limit = if self.async_turn_saw_usage {
                Duration::from_millis(500)
            } else {
                Duration::from_millis(1500)
            };
            if last_ev.elapsed() >= idle_limit {
                self.finish_async_turn();
                changed = true;
            }
        }
        changed
    }

    /// 标记服务端在 `busy == false` 期间主动推送了异步续答内容，启用底行流光动效并启动静默收尾计时。
    pub(super) fn mark_async_turn_activity(&mut self) {
        if self.busy || self.connecting {
            return;
        }
        if self.async_turn_last_event.is_none() {
            self.transcript.begin_turn();
            // 若当前展示的是非错误提示（如“后台任务已完成”），让位于实时流光状态行
            if self.toast.as_ref().is_some_and(|(_, is_err, _)| !*is_err) {
                self.toast = None;
            }
        }
        self.async_turn_last_event = Some(Instant::now());
        self.async_turn_saw_usage = false;
    }

    /// 结束当前异步续答回合，闭合未结束的思考块并写入 `▣` 轮次结束行。
    pub(super) fn finish_async_turn(&mut self) {
        if self.async_turn_last_event.take().is_none() {
            return;
        }
        self.async_turn_saw_usage = false;
        let label = self.turn_done_model_label();
        self.transcript.finish_turn(&label, "end_turn", None);
        self.flush_next_queued_message();
    }

    /// 组合写入轮次结束行 `▣` 的模型与思考等级标签。
    ///
    /// 返回:
    /// - 格式化后的模型标签字符串
    pub(super) fn turn_done_model_label(&self) -> String {
        let mut label = self.model_label();
        let effort = self.effort_label();
        if !effort.is_empty() {
            label.push_str(&format!(" ({effort})"));
        }
        label
    }

    pub fn model_label(&self) -> String {
        if let Some(opt) = self
            .config_options
            .iter()
            .find(|c| c.id == "model" || c.category == "model")
        {
            if let Some(choice) = opt.options.iter().find(|o| o.value == opt.current_value) {
                return choice.name.clone();
            }
            if !opt.current_value.is_empty() {
                return opt.current_value.clone();
            }
        }
        self.fallback_model_name.clone()
    }

    pub fn effort_label(&self) -> String {
        if let Some(opt) = self
            .config_options
            .iter()
            .find(|c| c.id == "thought_level" || c.category == "thought_level")
        {
            if let Some(choice) = opt.options.iter().find(|o| o.value == opt.current_value) {
                return choice.name.clone();
            }
            return opt.current_value.clone();
        }
        String::new()
    }

    pub fn mode_label(&self) -> String {
        if let Some(opt) = self
            .config_options
            .iter()
            .find(|c| c.id == "mode" || c.category == "mode")
        {
            if let Some(choice) = opt.options.iter().find(|o| o.value == opt.current_value) {
                return choice.name.clone();
            }
            return opt.current_value.clone();
        }
        self.modes
            .iter()
            .find(|m| m.id == self.current_mode)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| self.current_mode.clone())
    }

    /// 计算当前光标位置应展示的统一补全菜单（`/` 命令、`#` Skills、`@` 文件）。
    ///
    /// 返回:
    /// - 触发且未被本次 `Esc` 关闭时返回 `Some(CompletionMenu)`，否则返回 `None`
    pub fn active_completion_menu(&mut self) -> Option<CompletionMenu> {
        let full = self.editor.text().to_string();
        if let Some(dismissed) = &self.completion_dismissed_text {
            if dismissed == &full {
                return None;
            }
            self.completion_dismissed_text = None;
        }
        let before = self.editor.text_before_cursor().to_string();
        self.completion_cache.build_menu(
            &self.cwd,
            &full,
            &before,
            LOCAL_COMMANDS,
            &self.available_commands,
        )
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame) {
        let completion_menu = self.active_completion_menu();
        if let Some(menu) = &completion_menu {
            self.completion_selected = self
                .completion_selected
                .min(menu.items.len().saturating_sub(1));
        } else {
            self.completion_selected = 0;
        }
        let model = self.model_label();
        let effort = self.effort_label();
        let mode = self.mode_label();
        let toast = self.toast.as_ref().and_then(|(m, err, at)| {
            (at.elapsed() < Duration::from_secs(3)).then_some((m.as_str(), *err))
        });
        let permission = self.permission.as_ref().map(|(r, s)| (r, *s));
        let active_pending_images = self
            .pending_images
            .iter()
            .filter(|img| self.editor.text().contains(&img.label))
            .count();
        let background_tasks = self.background.running_tasks();
        let queued_head_preview = self.queued_messages.front().map(QueuedMessage::preview);
        let view = ViewState {
            transcript: &self.transcript,
            editor: &self.editor,
            cwd: &self.cwd,
            agent_banner: &self.agent_banner,
            model_label: &model,
            effort_label: &effort,
            mode_label: &mode,
            tier: self.tier.as_deref(),
            busy: self.is_generating(),
            connecting: self.connecting,
            scroll: self.scroll,
            auto_follow: self.auto_follow,
            show_sidebar: self.show_sidebar,
            tick: self.tick,
            started_at: self.started_at,
            toast,
            background_tasks: &background_tasks,
            subagents: &self.subagents.agents,
            recent_sessions: &self.recent_sessions,
            completion_menu: completion_menu.as_ref(),
            completion_selected: self.completion_selected,
            pending_images: active_pending_images,
            queued_mode: self.queued_mode,
            queued_count: self.queued_messages.len(),
            queued_head_preview: queued_head_preview.as_deref(),
            selection: self.selection,
            hover: &self.hover,
            picker: self.picker.as_ref(),
            permission,
            elicitation: self.elicitation.as_ref(),
        };
        let hits = crate::ui::draw(frame, &view, &mut self.md_cache);
        if self.auto_follow {
            self.scroll = hits
                .total_body_rows
                .saturating_sub(hits.visible_body_rows);
        } else {
            let max_s = hits
                .total_body_rows
                .saturating_sub(hits.visible_body_rows);
            self.scroll = self.scroll.min(max_s);
        }
        self.last_hits = hits;
    }

    pub(super) fn save_prefs(&self) {
        let prefs = UserPrefs {
            theme: theme::current_preset().id().to_string(),
            paint_bg: theme::paint_bg(),
            show_sidebar: self.show_sidebar,
            queued_mode: self.queued_mode,
        };
        prefs.save();
    }
}
