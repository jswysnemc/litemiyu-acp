//! 应用核心状态机：整合 ACP 异步事件、键盘/鼠标交互、斜杠命令与用户偏好持久化。

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use serde::{Deserialize, Serialize};

use crate::acp::{
    AcpClient, AcpCommand, AcpEvent, ConfigOption, ImagePayload, ModeInfo, PermissionRequest,
    SessionSummary, SessionUpdate, SlashCommandInfo, ToolDiff,
};
use crate::clipboard::{self, ClipboardContent, LoadedImage};
use crate::input::{AtomKind, InputEditor};
use crate::paste::{WindowsPasteOutcome, WindowsPasteState};
use crate::theme::{self, ThemePreset};
use crate::transcript::{ImageChip, Transcript, short_id};
use crate::ui::overlay::{
    PickerItem, PickerKind, PickerState, ResumeScope, SlashSuggestion, format_relative_time,
    parse_timestamp_secs, same_directory,
};
use crate::ui::rows::MdCache;
use crate::ui::{FrameHits, TextSelection, ViewState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPrefs {
    #[serde(default = "default_theme_id")]
    pub theme: String,
    #[serde(default = "default_true")]
    pub paint_bg: bool,
    #[serde(default = "default_true")]
    pub show_sidebar: bool,
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
    pub busy: bool,
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
    pub slash_selected: usize,
    pub pending_images: Vec<PendingImage>,
    pub image_seq: usize,
    pub windows_paste: WindowsPasteState,
    pub selection: Option<TextSelection>,
    pub dragging: bool,
    pub dragging_scrollbar: bool,
    pub picker: Option<PickerState>,
    pub permission: Option<(PermissionRequest, usize)>,
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
            busy: false,
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
            slash_selected: 0,
            pending_images: Vec::new(),
            image_seq: 0,
            windows_paste: WindowsPasteState::default(),
            selection: None,
            dragging: false,
            dragging_scrollbar: false,
            picker: None,
            permission: None,
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
             > 注：点击上方 `◇ 思考` 或 `✓ 读取` 可原地展开细节；输入 `/theme` 可实时切换 5 套配色主题。",
        );
        self.transcript
            .finish_turn("Gemini 4 Neon · High", "end_turn", None);
    }

    pub fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
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

    pub fn slash_suggestions(&self) -> Vec<SlashSuggestion> {
        let text = self.editor.text();
        if !text.starts_with('/') || text.contains('\n') || text.contains(' ') {
            return Vec::new();
        }
        let q = text[1..].to_ascii_lowercase();
        let mut out = Vec::new();
        for &(name, desc) in LOCAL_COMMANDS {
            if q.is_empty()
                || name.contains(&q)
                || desc.to_ascii_lowercase().contains(&q)
            {
                out.push(SlashSuggestion {
                    name: name.to_string(),
                    description: desc.to_string(),
                    is_local: true,
                });
            }
        }
        for cmd in &self.available_commands {
            if LOCAL_COMMANDS.iter().any(|(n, _)| *n == cmd.name) {
                continue;
            }
            if q.is_empty()
                || cmd.name.to_ascii_lowercase().contains(&q)
                || cmd.description.to_ascii_lowercase().contains(&q)
            {
                out.push(SlashSuggestion {
                    name: cmd.name.clone(),
                    description: cmd.description.clone(),
                    is_local: false,
                });
            }
        }
        out
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame) {
        let slash_menu = self.slash_suggestions();
        if !slash_menu.is_empty() {
            self.slash_selected = self.slash_selected.min(slash_menu.len() - 1);
        } else {
            self.slash_selected = 0;
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
        let view = ViewState {
            transcript: &self.transcript,
            editor: &self.editor,
            cwd: &self.cwd,
            agent_banner: &self.agent_banner,
            model_label: &model,
            effort_label: &effort,
            mode_label: &mode,
            tier: self.tier.as_deref(),
            busy: self.busy,
            connecting: self.connecting,
            scroll: self.scroll,
            auto_follow: self.auto_follow,
            show_sidebar: self.show_sidebar,
            tick: self.tick,
            started_at: self.started_at,
            toast,
            recent_sessions: &self.recent_sessions,
            slash_menu: &slash_menu,
            slash_selected: self.slash_selected,
            pending_images: active_pending_images,
            selection: self.selection,
            picker: self.picker.as_ref(),
            permission,
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

    pub fn handle_acp_event(&mut self, ev: AcpEvent) {
        match ev {
            AcpEvent::Initialized {
                agent_name,
                agent_version,
            } => {
                self.agent_banner = if agent_version.is_empty() {
                    agent_name
                } else {
                    format!("{agent_name} v{agent_version}")
                };
                if let Some(client) = &self.client {
                    client.send(AcpCommand::ListSessions { cwd: None });
                    if let Some(sid) = self.pending_resume.take() {
                        client.send(AcpCommand::LoadSession {
                            session_id: sid,
                            cwd: self.cwd.clone(),
                        });
                    } else {
                        client.send(AcpCommand::NewSession {
                            cwd: self.cwd.clone(),
                        });
                    }
                }
            }
            AcpEvent::SessionReady {
                session_id,
                config_options,
                modes,
                current_mode,
                current_model_name,
                tier,
                is_load,
            } => {
                self.connecting = false;
                self.transcript.session_id = Some(session_id.clone());
                if !config_options.is_empty() {
                    self.config_options = config_options;
                }
                if !modes.is_empty() {
                    self.modes = modes;
                }
                if let Some(m) = current_mode {
                    self.current_mode = m;
                }
                if let Some(name) = current_model_name {
                    self.fallback_model_name = name;
                }
                if tier.is_some() {
                    self.tier = tier;
                }
                if is_load {
                    self.toast(format!("已恢复会话 #{}", short_id(&session_id)), false);
                    self.auto_follow = true;
                }
                if let Some(client) = &self.client {
                    client.send(AcpCommand::ListSessions { cwd: None });
                }
            }
            AcpEvent::SessionsListed(mut list) => {
                list.sort_by(|a, b| {
                    let ta = parse_timestamp_secs(&a.updated_at).unwrap_or(0);
                    let tb = parse_timestamp_secs(&b.updated_at).unwrap_or(0);
                    tb.cmp(&ta)
                });
                self.recent_sessions = list;
                self.refresh_resume_picker_items();
            }
            AcpEvent::Update { update, .. } => self.handle_session_update(update),
            AcpEvent::PermissionRequested(req) => {
                self.permission = Some((req, 0));
            }
            AcpEvent::PromptFinished { stop_reason, usage } => {
                self.busy = false;
                self.permission = None;
                let mut label = self.model_label();
                let effort = self.effort_label();
                if !effort.is_empty() {
                    label.push_str(&format!(" ({effort})"));
                }
                self.transcript
                    .finish_turn(&label, &stop_reason, usage.as_ref());
            }
            AcpEvent::ConfigUpdated(opts) => {
                if !opts.is_empty() {
                    self.config_options = opts;
                    if let Some(m) = self
                        .config_options
                        .iter()
                        .find(|c| c.id == "mode" || c.category == "mode")
                    {
                        self.current_mode = m.current_value.clone();
                    }
                }
            }
            AcpEvent::Error(msg) => {
                self.connecting = false;
                self.transcript.push_notice(msg.clone(), true);
                self.toast(msg, true);
            }
            AcpEvent::Disconnected(reason) => {
                self.connecting = false;
                self.busy = false;
                self.transcript.push_notice(reason.clone(), true);
                self.toast(reason, true);
            }
        }
    }

    fn handle_session_update(&mut self, update: SessionUpdate) {
        match update {
            SessionUpdate::UserMessageChunk(text) => {
                self.transcript.append_user_chunk(&text);
            }
            SessionUpdate::AgentThoughtChunk(text) => {
                self.transcript.append_thought(&text);
            }
            SessionUpdate::AgentMessageChunk(text) => {
                self.transcript.append_assistant(&text);
            }
            SessionUpdate::ToolCall {
                call_id,
                name,
                title,
                kind,
                status,
                raw_input,
                output,
                diffs,
            } => {
                self.transcript.upsert_tool(
                    call_id,
                    Some(name),
                    Some(title),
                    Some(kind),
                    Some(status),
                    raw_input,
                    output,
                    diffs,
                );
            }
            SessionUpdate::ToolCallUpdate {
                call_id,
                name,
                title,
                kind,
                status,
                raw_input,
                output,
                diffs,
            } => {
                self.transcript.upsert_tool(
                    call_id, name, title, kind, status, raw_input, output, diffs,
                );
            }
            SessionUpdate::Plan(items) => {
                self.transcript.update_plan(items);
            }
            SessionUpdate::AvailableCommands(cmds) => {
                self.available_commands = cmds;
            }
            SessionUpdate::ConfigOptions(opts) => {
                if !opts.is_empty() {
                    self.config_options = opts;
                }
            }
            SessionUpdate::CurrentMode(mode) => {
                self.current_mode = mode;
            }
            SessionUpdate::SessionTitle(title) => {
                if !title.trim().is_empty() {
                    self.transcript.title = Some(title);
                }
            }
            SessionUpdate::Usage { used, size } => {
                self.transcript.context_used = used;
                if size > 0 {
                    self.transcript.context_window = size;
                }
            }
        }
    }

    #[cfg(test)]
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.handle_key_queued(key, false);
    }

    pub fn handle_key_queued(&mut self, key: KeyEvent, has_more_queued_keys: bool) {
        // Windows crossterm 会同时上报 Release 事件，Kitty 增强协议可能上报裸修饰键，统一忽略
        if key.kind == KeyEventKind::Release || matches!(key.code, KeyCode::Modifier(_)) {
            return;
        }

        // 1. 权限审批抽屉优先
        if self.permission.is_some() {
            self.windows_paste.reset();
            self.handle_permission_key(key);
            return;
        }

        // 2. 模态选择器优先
        if self.picker.is_some() {
            self.windows_paste.reset();
            self.handle_picker_key(key);
            return;
        }

        // 3. 全局快捷键
        if is_ctrl_char(key.code, key.modifiers, 'g')
            || (matches!(key.code, KeyCode::Char('g' | 'G'))
                && key.modifiers == KeyModifiers::ALT)
        {
            self.windows_paste.reset();
            self.selection = None;
            self.dragging = false;
            self.external_editor_pending = true;
            return;
        }
        if is_ctrl_char(key.code, key.modifiers, 'a') {
            self.windows_paste.reset();
            self.selection = None;
            self.dragging = false;
            self.editor.move_to_start();
            return;
        }
        if is_ctrl_char(key.code, key.modifiers, 'e') {
            self.windows_paste.reset();
            self.selection = None;
            self.dragging = false;
            self.editor.move_to_end();
            return;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                // 若存在鼠标划词选区，Ctrl+C 直接复制选区并取消高亮
                if let Some(sel) = self.selection.take() {
                    let text = self.last_hits.extract_selection(sel);
                    if !text.is_empty() {
                        let count = text.chars().count();
                        clipboard::copy_text(&text);
                        self.toast(format!("已复制选中内容 ({count} 字符)"), false);
                        return;
                    }
                }
                if self.busy {
                    self.cancel_turn();
                } else if !self.editor.is_empty() || !self.pending_images.is_empty() {
                    self.editor.clear();
                    self.pending_images.clear();
                    self.toast("已清空输入框（再按一次 Ctrl+C 退出）", false);
                    self.last_ctrl_c = Some(Instant::now());
                } else if self
                    .last_ctrl_c
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
                {
                    self.should_quit = true;
                } else {
                    self.last_ctrl_c = Some(Instant::now());
                    self.toast("再按一次 Ctrl+C 退出", false);
                }
                return;
            }
            (KeyCode::Char('v'), KeyModifiers::CONTROL)
            | (KeyCode::Char('v'), KeyModifiers::ALT) => {
                self.windows_paste.reset();
                self.paste_from_clipboard(false);
                return;
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                if self.editor.is_empty() {
                    self.should_quit = true;
                } else if let Some(atom) = self.editor.delete() {
                    if atom.kind == AtomKind::Image {
                        self.prune_pending_images();
                    }
                }
                return;
            }
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.show_sidebar = !self.show_sidebar;
                self.save_prefs();
                self.toast(
                    if self.show_sidebar {
                        "已开启右侧边栏"
                    } else {
                        "已隐藏右侧边栏"
                    },
                    false,
                );
                return;
            }
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.all_expanded = !self.all_expanded;
                self.transcript.toggle_all_details(self.all_expanded);
                self.toast(
                    if self.all_expanded {
                        "已展开全部思考与工具详情"
                    } else {
                        "已折叠全部思考与工具详情"
                    },
                    false,
                );
                return;
            }
            (KeyCode::BackTab, _) => {
                self.windows_paste.reset();
                self.cycle_mode();
                return;
            }
            (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => {
                self.windows_paste.reset();
                self.cycle_mode();
                return;
            }
            (KeyCode::PageUp, _) => {
                self.windows_paste.reset();
                self.scroll_up(12);
                return;
            }
            (KeyCode::PageDown, _) => {
                self.windows_paste.reset();
                self.scroll_down(12);
                return;
            }
            (KeyCode::Up, KeyModifiers::ALT) => {
                self.windows_paste.reset();
                self.scroll_up(3);
                return;
            }
            (KeyCode::Down, KeyModifiers::ALT) => {
                self.windows_paste.reset();
                self.scroll_down(3);
                return;
            }
            (KeyCode::End, m) if self.editor.is_empty() || m.contains(KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.auto_follow = true;
                return;
            }
            (KeyCode::Home, m) if m.contains(KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.auto_follow = false;
                self.scroll = 0;
                return;
            }
            (KeyCode::Esc, _) => {
                self.windows_paste.reset();
                if self.selection.take().is_some() {
                    self.dragging = false;
                } else if self.busy {
                    self.cancel_turn();
                } else if !self.editor.is_empty() || !self.pending_images.is_empty() {
                    self.editor.clear();
                    self.pending_images.clear();
                }
                return;
            }
            _ => {}
        }

        // 3.5 Windows 无括号多行粘贴检测与回放吞并（必须在斜杠菜单与 Enter 发送之前拦截）
        if cfg!(windows) || has_more_queued_keys {
            match self.windows_paste.handle_key(
                key,
                self.editor.text_before_cursor(),
                has_more_queued_keys,
                Instant::now(),
                clipboard::read_clipboard_text,
            ) {
                WindowsPasteOutcome::ConsumedReplay => {
                    self.windows_paste.drain_console_replay();
                    return;
                }
                WindowsPasteOutcome::ReplacePrefixWithPaste {
                    prefix_bytes,
                    full_text,
                } => {
                    self.editor.delete_bytes_before_cursor(prefix_bytes);
                    self.insert_pasted_text(full_text);
                    self.windows_paste.drain_console_replay();
                    return;
                }
                WindowsPasteOutcome::InsertBurstChar(ch) => {
                    self.editor.insert_char(ch);
                    return;
                }
                WindowsPasteOutcome::NotPaste => {}
            }
        }

        // 4. 斜杠补全菜单导航
        let suggestions = self.slash_suggestions();
        if !suggestions.is_empty() {
            match key.code {
                KeyCode::Up => {
                    self.slash_selected = if self.slash_selected == 0 {
                        suggestions.len() - 1
                    } else {
                        self.slash_selected - 1
                    };
                    return;
                }
                KeyCode::Down => {
                    self.slash_selected = (self.slash_selected + 1) % suggestions.len();
                    return;
                }
                KeyCode::Tab => {
                    if let Some(item) = suggestions.get(self.slash_selected) {
                        self.editor.set_text(format!("/{} ", item.name));
                    }
                    return;
                }
                KeyCode::Enter if key.modifiers.is_empty() => {
                    if let Some(item) = suggestions.get(self.slash_selected) {
                        let cmd = format!("/{}", item.name);
                        self.editor.clear();
                        self.submit_text(cmd);
                    }
                    return;
                }
                _ => {}
            }
        }

        // 5. 普通输入框编辑（开始输入时清除旧的划词高亮）
        if self.selection.is_some() {
            self.selection = None;
            self.dragging = false;
        }
        let input_w = self.last_hits.input_text_width.max(16);
        match (key.code, key.modifiers) {
            (KeyCode::Enter, m)
                if m.contains(KeyModifiers::SHIFT)
                    || m.contains(KeyModifiers::ALT)
                    || m.contains(KeyModifiers::CONTROL) =>
            {
                self.editor.insert_char('\n');
            }
            (KeyCode::Char('\n'), _)
            | (KeyCode::Char('j' | 'J'), KeyModifiers::CONTROL) => {
                self.editor.insert_char('\n');
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                self.windows_paste.reset();
                let (_raw, expanded) = self.editor.take_and_record();
                if !expanded.trim().is_empty() || !self.pending_images.is_empty() {
                    self.submit_text(expanded);
                }
            }
            (KeyCode::Backspace, _) => {
                if let Some(atom) = self.editor.backspace() {
                    if atom.kind == AtomKind::Image {
                        self.prune_pending_images();
                    }
                }
            }
            (KeyCode::Delete, _) => {
                if let Some(atom) = self.editor.delete() {
                    if atom.kind == AtomKind::Image {
                        self.prune_pending_images();
                    }
                }
            }
            (KeyCode::Left, _) => self.editor.move_left(),
            (KeyCode::Right, _) => self.editor.move_right(),
            (KeyCode::Home, _) => self.editor.move_home(),
            (KeyCode::End, _) => self.editor.move_end(),
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                self.editor.delete_word_back();
                self.prune_pending_images();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.editor.delete_to_start();
                self.prune_pending_images();
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.editor.delete_to_end();
                self.prune_pending_images();
            }
            (KeyCode::Up, _) => {
                if !self.editor.move_up(input_w) {
                    self.editor.history_prev();
                }
            }
            (KeyCode::Down, _) => {
                if !self.editor.move_down(input_w) {
                    self.editor.history_next();
                }
            }
            (KeyCode::Char(c), m)
                if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
            {
                self.editor.insert_char(c);
            }
            _ => {}
        }
    }

    /// 准备写入外部编辑器（`$EDITOR` / `notepad.exe`）的输入正文（展开大段粘贴块）。
    pub fn prepare_external_editor_buffer(&self) -> String {
        self.editor.expanded_text()
    }

    /// 外部编辑器退出后，将编辑后的正文替换回输入框并同步待发送图片附件。
    pub fn apply_external_editor_result(&mut self, result: Result<String, String>) {
        self.windows_paste.reset();
        match result {
            Ok(edited) => {
                let prev = self.editor.expanded_text();
                let markers: Vec<String> = self
                    .pending_images
                    .iter()
                    .map(|img| img.label.clone())
                    .collect();
                self.editor.replace_from_external(edited.clone(), &markers);
                self.prune_pending_images();
                if edited == prev {
                    self.toast("外部编辑器未修改内容", false);
                } else {
                    let chars = edited.chars().count();
                    self.toast(format!("已从外部编辑器替换输入 ({chars} 字符)"), false);
                }
            }
            Err(err) => {
                self.toast(err, true);
            }
        }
    }

    pub fn handle_paste(&mut self, text: String) {
        self.windows_paste.reset();
        if let Some(picker) = &mut self.picker {
            if picker.kind != PickerKind::Help {
                picker.query.push_str(text.trim());
                picker.selected = 0;
            }
            return;
        }
        if self.permission.is_some() {
            return;
        }
        // 部分终端在剪贴板为图片时会发出空 Bracketed Paste 事件，趁机主动探测系统剪贴板图片
        if text.is_empty() {
            self.paste_from_clipboard(true);
            return;
        }
        // 拖拽图片进终端或粘贴图片文件路径时自动加载为图片芯片
        if let Some(imgs) = clipboard::try_load_pasted_image_paths(&text) {
            self.attach_images(imgs);
            return;
        }
        self.insert_pasted_text(text);
    }

    fn insert_pasted_text(&mut self, text: String) {
        if let Some(imgs) = clipboard::try_load_pasted_image_paths(&text) {
            self.attach_images(imgs);
            return;
        }
        if let Some((_marker, lines, chars)) = self.editor.insert_pasted_text(&text) {
            self.toast(
                format!("已粘贴为原子文本块 ({lines} 行 / {chars} 字，退格一键删除)"),
                false,
            );
        }
    }

    fn paste_from_clipboard(&mut self, image_only: bool) {
        match clipboard::read_clipboard(image_only) {
            ClipboardContent::Images(imgs) => self.attach_images(imgs),
            ClipboardContent::Text(s) => self.insert_pasted_text(s),
            ClipboardContent::Empty => {
                self.toast("剪贴板中没有图片或文字", true);
            }
        }
    }

    fn prune_pending_images(&mut self) {
        let current = self.editor.text();
        self.pending_images
            .retain(|img| current.contains(&img.label));
    }

    fn attach_images(&mut self, imgs: Vec<LoadedImage>) {
        if imgs.is_empty() {
            return;
        }
        let mut labels = Vec::new();
        let mut total_kb = 0usize;
        for img in imgs {
            self.image_seq += 1;
            let label = format!("[图片 {}]", self.image_seq);
            total_kb += (img.byte_len + 1023) / 1024;
            self.editor.insert_image_atom(label.clone());
            labels.push(label.clone());
            self.pending_images.push(PendingImage { label, image: img });
        }
        self.toast(
            format!("已附加 {} ({total_kb} KB，退格一键删除)", labels.join(" ")),
            false,
        );
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.picker.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(p) = &mut self.picker {
                        p.move_up();
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(p) = &mut self.picker {
                        p.move_down();
                    }
                }
                _ => {}
            }
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.selection = None;
                self.dragging = false;
                self.scroll_up(3);
            }
            MouseEventKind::ScrollDown => {
                self.selection = None;
                self.dragging = false;
                self.scroll_down(3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                // 1. 点击「↓ 回到底部」按钮：立即回到最底部并恢复自动跟随
                if rect_contains(self.last_hits.bottom_button, col, row) {
                    self.selection = None;
                    self.dragging = false;
                    self.dragging_scrollbar = false;
                    self.jump_to_bottom();
                    return;
                }
                // 2. 点击/按住正文右侧滚动条：立即按比例定位并开始拖动
                if rect_contains(self.last_hits.scrollbar, col, row) {
                    self.selection = None;
                    self.dragging = false;
                    self.dragging_scrollbar = true;
                    self.scroll_to_scrollbar_row(row);
                    return;
                }
                let zone = self.selection_zone(col, row);
                self.selection = Some(TextSelection {
                    anchor: (col, row),
                    head: (col, row),
                    zone,
                });
                self.dragging = false;
                self.dragging_scrollbar = false;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                if self.dragging_scrollbar {
                    self.scroll_to_scrollbar_row(row);
                    return;
                }
                if let Some(sel) = &mut self.selection {
                    if (col, row) != sel.anchor {
                        self.dragging = true;
                    }
                    sel.head = (col, row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                if self.dragging_scrollbar {
                    self.dragging_scrollbar = false;
                    self.scroll_to_scrollbar_row(row);
                    return;
                }
                if self.dragging {
                    self.dragging = false;
                    if let Some(mut sel) = self.selection {
                        sel.head = (col, row);
                        self.selection = Some(sel);
                        let extracted = self.last_hits.extract_selection(sel);
                        if !extracted.is_empty() {
                            let chars = extracted.chars().count();
                            clipboard::copy_text(&extracted);
                            self.toast(format!("已复制选中内容 ({chars} 字符)"), false);
                        } else {
                            self.selection = None;
                        }
                    }
                    return;
                }

                // 单击（非拖拽）：取消选区并处理按钮/链接/折叠/侧边栏点击
                self.selection = None;
                self.handle_left_click(col, row);
            }
            _ => {}
        }
    }

    fn selection_zone(&self, col: u16, row: u16) -> ratatui::layout::Rect {
        // 1. 侧边栏区域（包含左侧分隔竖线列，均自动限定到纯文字区，不带入 `│`）
        let side = self.last_hits.sidebar.area;
        if side.width > 2 && col >= side.x && col < side.right() {
            return ratatui::layout::Rect::new(
                side.x + 2,
                side.y,
                side.width.saturating_sub(2),
                side.height,
            );
        }
        // 2. 输入框区域：仅选中内部纯文字区，不带入圆角边框 `│`/`╭`/`╰`、`❯` 与迷你滚动条
        let input_box = self.last_hits.input_area;
        if input_box.width > 0
            && self.last_hits.input_text_area.width > 0
            && rect_contains(input_box, col, row)
        {
            return self.last_hits.input_text_area;
        }
        // 3. 主对话栏正文区（含左右留白边距、顶部空行与状态行起手拖拽），严格限定在纯文字区，绝不带入右侧滚动条 `┃`/`│` 与边栏竖线 `│`
        let body_text = self.last_hits.body_text;
        let main_area = self.last_hits.main_area;
        let in_main_x = if main_area.width > 0 {
            col >= main_area.x && col < main_area.right()
        } else {
            side.width == 0 || col < side.x
        };
        if body_text.width > 0 && in_main_x && (input_box.height == 0 || row < input_box.y) {
            return body_text;
        }
        // 4. 其余区域兜底（如底栏/首页），同样横向限定在主栏内避免越过侧边栏竖线
        if main_area.width > 0 && main_area.height > 0 {
            return main_area;
        }
        let rows = u16::try_from(self.last_hits.screen_lines.len()).unwrap_or(1);
        let cols = self
            .last_hits
            .screen_lines
            .first()
            .map_or(1, |l| u16::try_from(l.len()).unwrap_or(1));
        ratatui::layout::Rect::new(0, 0, cols, rows)
    }

    fn handle_left_click(&mut self, col: u16, row: u16) {
        // 0. 点击「回到底部」/ 底栏用量格 / 侧边栏上下文进度条
        if rect_contains(self.last_hits.bottom_button, col, row) {
            self.jump_to_bottom();
            return;
        }
        if rect_contains(self.last_hits.usage_cell, col, row)
            || rect_contains(self.last_hits.sidebar.context_area, col, row)
        {
            self.open_status_picker();
            return;
        }

        // 1. 点击侧边栏跳转消息 / 切换最近会话
        if self.last_hits.sidebar.area.width > 0
            && col >= self.last_hits.sidebar.area.x
            && col < self.last_hits.sidebar.area.right()
        {
            if let Some((_, eid)) = self
                .last_hits
                .sidebar
                .jump_rows
                .iter()
                .find(|(y, _)| *y == row)
            {
                if let Some(&target_row) = self.last_hits.entry_starts.get(eid) {
                    self.auto_follow = false;
                    self.scroll = target_row;
                }
                return;
            }
            if let Some((_, sid)) = self
                .last_hits
                .sidebar
                .session_rows
                .iter()
                .find(|(y, _)| *y == row)
                .cloned()
            {
                self.load_session(sid);
                return;
            }
        }

        // 2. 点击正文区域：优先打开链接，其次展开/折叠思考与工具块
        if col >= self.last_hits.body.x && col < self.last_hits.body.right() {
            let rel_col = col.saturating_sub(self.last_hits.body.x);
            if let Some((_, eid, clickable, links)) = self
                .last_hits
                .body_rows
                .iter()
                .find(|(y, _, _, _)| *y == row)
                .cloned()
            {
                if let Some((_, _, url)) = links
                    .iter()
                    .find(|(a, b, _)| rel_col >= *a && rel_col < *b)
                {
                    clipboard::open_target(url);
                    self.toast(format!("已打开 {url}"), false);
                    return;
                }
                if clickable {
                    self.transcript.toggle_entry(eid);
                }
            }
        }
    }

    fn handle_permission_key(&mut self, key: KeyEvent) {
        let Some((req, sel)) = &mut self.permission else {
            return;
        };
        let len = req.options.len();
        match key.code {
            KeyCode::Up => {
                if len > 0 {
                    *sel = if *sel == 0 { len - 1 } else { *sel - 1 };
                }
            }
            KeyCode::Down => {
                if len > 0 {
                    *sel = (*sel + 1) % len;
                }
            }
            KeyCode::Enter => {
                let opt_id = req.options.get(*sel).map(|o| o.option_id.clone());
                self.answer_permission(opt_id);
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let opt_id = req
                    .options
                    .iter()
                    .find(|o| o.kind.contains("allow"))
                    .or_else(|| req.options.first())
                    .map(|o| o.option_id.clone());
                self.answer_permission(opt_id);
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                let opt_id = req
                    .options
                    .iter()
                    .find(|o| o.kind.contains("reject"))
                    .map(|o| o.option_id.clone());
                self.answer_permission(opt_id);
            }
            KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                let idx = (c as usize) - ('1' as usize);
                if let Some(opt) = req.options.get(idx) {
                    let id = opt.option_id.clone();
                    self.answer_permission(Some(id));
                }
            }
            _ => {}
        }
    }

    fn answer_permission(&mut self, option_id: Option<String>) {
        if let Some((req, _)) = self.permission.take() {
            if let Some(client) = &self.client {
                client.send(AcpCommand::AnswerPermission {
                    rpc_id: req.rpc_id,
                    option_id,
                });
            }
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.picker = None;
            }
            KeyCode::Tab | KeyCode::BackTab if picker.kind == PickerKind::Resume => {
                picker.toggle_scope();
            }
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::PageUp => picker.page_up(5),
            KeyCode::PageDown => picker.page_down(5),
            KeyCode::Backspace
                if !matches!(picker.kind, PickerKind::Help | PickerKind::Status) =>
            {
                picker.query.pop();
                picker.selected = 0;
            }
            KeyCode::Char(c)
                if !matches!(picker.kind, PickerKind::Help | PickerKind::Status)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                picker.query.push(c);
                picker.selected = 0;
            }
            KeyCode::Enter => {
                let kind = picker.kind;
                let chosen = picker.selected_item().cloned();
                self.picker = None;
                if let Some(item) = chosen {
                    self.apply_picker_choice(kind, item);
                }
            }
            _ => {}
        }
    }

    fn apply_picker_choice(&mut self, kind: PickerKind, item: PickerItem) {
        match kind {
            PickerKind::Model => {
                self.set_config_option("model", &item.id);
                self.toast(format!("已切换模型：{}", item.label), false);
            }
            PickerKind::Effort => {
                self.set_config_option("thought_level", &item.id);
                self.toast(format!("已切换思考等级：{}", item.label), false);
            }
            PickerKind::Mode => {
                self.set_mode(&item.id);
                self.toast(format!("已切换模式：{}", item.label), false);
            }
            PickerKind::Resume => {
                self.load_session(item.id);
            }
            PickerKind::Theme => {
                if let Some(preset) = ThemePreset::parse(&item.id) {
                    theme::set_preset(preset);
                    self.save_prefs();
                    self.toast(format!("已切换主题：{}", preset.label()), false);
                }
            }
            PickerKind::Status => match item.id.as_str() {
                "compact" => self.submit_text("/compact".into()),
                "copy_id" => {
                    if let Some(sid) = self.transcript.session_id.clone() {
                        clipboard::copy_text(&sid);
                        self.toast(format!("已复制会话 ID：{sid}"), false);
                    }
                }
                "model" => self.open_config_picker("model", "选择模型", PickerKind::Model),
                _ => {}
            },
            PickerKind::Help => {}
        }
    }

    fn submit_text(&mut self, text: String) {
        let trimmed = text.trim();
        let pending = std::mem::take(&mut self.pending_images);
        let attached: Vec<PendingImage> = pending
            .into_iter()
            .filter(|p| text.contains(&p.label) || trimmed.is_empty())
            .collect();

        if trimmed.is_empty() && attached.is_empty() {
            return;
        }
        if attached.is_empty() {
            if let Some(rest) = trimmed.strip_prefix('/') {
                let mut parts = rest.splitn(2, char::is_whitespace);
                let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
                let arg = parts.next().unwrap_or("").trim();
                if self.try_local_command(&cmd, arg) {
                    return;
                }
            }
        }

        if self.busy {
            self.pending_images = attached;
            self.toast("当前正在生成中，请稍候或按 Esc 中断", true);
            return;
        }
        let Some(sid) = self.transcript.session_id.clone() else {
            self.pending_images = attached;
            self.toast("ACP 会话尚未就绪，请稍候", true);
            return;
        };

        let display_text = if trimmed.is_empty() {
            attached
                .iter()
                .map(|a| a.label.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            text.clone()
        };
        let chips: Vec<ImageChip> = attached
            .iter()
            .map(|a| ImageChip {
                label: a.label.clone(),
                path: a.image.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            })
            .collect();
        let payloads: Vec<ImagePayload> = attached
            .into_iter()
            .map(|a| ImagePayload {
                mime_type: a.image.mime_type,
                base64_data: a.image.base64_data,
            })
            .collect();

        self.transcript.push_user(display_text, chips);
        self.transcript.turn_started = Some(Instant::now());
        self.busy = true;
        self.auto_follow = true;
        if let Some(client) = &self.client {
            client.send(AcpCommand::Prompt {
                session_id: sid,
                text,
                images: payloads,
            });
        } else {
            self.busy = false;
            self.transcript
                .push_notice("当前为 --demo 离线预览模式，去掉 --demo 即可连接 agy acp", false);
        }
    }

    fn try_local_command(&mut self, cmd: &str, arg: &str) -> bool {
        match cmd {
            "quit" | "exit" | "q" => {
                self.should_quit = true;
                true
            }
            "model" => {
                self.open_config_picker("model", "选择模型", PickerKind::Model);
                true
            }
            "effort" => {
                self.open_config_picker("thought_level", "选择思考等级", PickerKind::Effort);
                true
            }
            "mode" => {
                self.open_mode_picker();
                true
            }
            "resume" => {
                self.open_resume_picker();
                true
            }
            "new" | "clear" => {
                self.start_new_session();
                true
            }
            "image" | "img" | "paste" => {
                if arg.is_empty() {
                    self.paste_from_clipboard(true);
                } else {
                    let cleaned = arg.trim_matches(|c| c == '"' || c == '\'');
                    match clipboard::load_image_file(std::path::Path::new(cleaned)) {
                        Ok(img) => self.attach_images(vec![img]),
                        Err(err) => self.toast(err, true),
                    }
                }
                true
            }
            "theme" => {
                if !arg.is_empty() {
                    if let Some(preset) = ThemePreset::parse(arg) {
                        theme::set_preset(preset);
                        self.save_prefs();
                        self.toast(format!("已切换主题：{}", preset.label()), false);
                        return true;
                    }
                }
                self.open_theme_picker();
                true
            }
            "bg" => {
                let on = theme::toggle_bg();
                self.save_prefs();
                self.toast(
                    if on {
                        "已开启主题铺底色"
                    } else {
                        "已切换为透明背景（继承终端）"
                    },
                    false,
                );
                true
            }
            "sidebar" => {
                self.show_sidebar = !self.show_sidebar;
                self.save_prefs();
                self.toast(
                    if self.show_sidebar {
                        "已开启右侧边栏"
                    } else {
                        "已隐藏右侧边栏"
                    },
                    false,
                );
                true
            }
            "copy" => {
                if arg.eq_ignore_ascii_case("all") {
                    let md = self.transcript.full_conversation_markdown();
                    if md.is_empty() {
                        self.toast("当前会话暂无可复制内容", true);
                    } else {
                        let chars = md.chars().count();
                        clipboard::copy_text(&md);
                        self.toast(format!("已复制完整对话记录 ({chars} 字符)"), false);
                    }
                } else {
                    let n = arg.parse::<usize>().unwrap_or(1).max(1);
                    if let Some(text) = self.transcript.nth_assistant_text(n) {
                        let chars = text.chars().count();
                        clipboard::copy_text(text);
                        self.toast(format!("已复制第 {n} 条（倒数）Agent 回答 ({chars} 字符)"), false);
                    } else {
                        self.toast("暂无可复制的回答", true);
                    }
                }
                true
            }
            "mouse" => {
                self.mouse_capture = !self.mouse_capture;
                self.mouse_toggle_pending = Some(self.mouse_capture);
                self.toast(
                    if self.mouse_capture {
                        "已开启鼠标滚轮与点击交互（按住 Shift 可终端划词）"
                    } else {
                        "已关闭鼠标捕获（可直接终端划词，PgUp/PgDn 翻页）"
                    },
                    false,
                );
                true
            }
            "help" => {
                self.open_help_picker();
                true
            }
            _ => false,
        }
    }

    fn open_config_picker(&mut self, config_id: &str, title: &str, kind: PickerKind) {
        let Some(opt) = self
            .config_options
            .iter()
            .find(|c| c.id == config_id || c.category == config_id)
        else {
            self.toast(format!("当前 ACP 后端未提供 {title} 选项"), true);
            return;
        };
        let items: Vec<PickerItem> = opt
            .options
            .iter()
            .map(|o| {
                PickerItem::simple(
                    o.value.clone(),
                    o.name.clone(),
                    if o.description.is_empty() {
                        o.value.clone()
                    } else {
                        o.description.clone()
                    },
                    o.value == opt.current_value,
                )
            })
            .collect();
        let selected = items.iter().position(|i| i.active).unwrap_or(0);
        self.picker = Some(PickerState::new(kind, title, items, selected));
    }

    fn open_mode_picker(&mut self) {
        if self
            .config_options
            .iter()
            .any(|c| c.id == "mode" || c.category == "mode")
        {
            self.open_config_picker("mode", "切换执行模式", PickerKind::Mode);
            return;
        }
        let items: Vec<PickerItem> = self
            .modes
            .iter()
            .map(|m| {
                PickerItem::simple(
                    m.id.clone(),
                    m.name.clone(),
                    m.description.clone(),
                    m.id == self.current_mode,
                )
            })
            .collect();
        if items.is_empty() {
            self.toast("当前 ACP 后端未提供模式列表", true);
            return;
        }
        let selected = items.iter().position(|i| i.active).unwrap_or(0);
        self.picker = Some(PickerState::new(
            PickerKind::Mode,
            "切换执行模式",
            items,
            selected,
        ));
    }

    fn build_resume_items(&self) -> Vec<PickerItem> {
        let cur = self.transcript.session_id.as_deref();
        let mut sessions: Vec<&SessionSummary> = self.recent_sessions.iter().collect();
        sessions.sort_by(|a, b| {
            let ta = parse_timestamp_secs(&a.updated_at).unwrap_or(0);
            let tb = parse_timestamp_secs(&b.updated_at).unwrap_or(0);
            tb.cmp(&ta)
        });
        sessions
            .into_iter()
            .map(|s| {
                let title = if s.title.trim().is_empty() {
                    format!("未命名会话 #{}", short_id(&s.session_id))
                } else {
                    s.title.clone()
                };
                PickerItem {
                    id: s.session_id.clone(),
                    label: title,
                    detail: s.cwd.clone(),
                    meta: format_relative_time(&s.updated_at),
                    same_cwd: same_directory(&s.cwd, &self.cwd),
                    active: cur == Some(s.session_id.as_str()),
                }
            })
            .collect()
    }

    fn refresh_resume_picker_items(&mut self) {
        let is_resume_open = self
            .picker
            .as_ref()
            .is_some_and(|p| p.kind == PickerKind::Resume);
        if !is_resume_open {
            return;
        }
        let items = self.build_resume_items();
        if let Some(picker) = &mut self.picker {
            let prev_empty_cur = picker.current_dir_count() == 0;
            picker.items = items;
            if prev_empty_cur && picker.current_dir_count() > 0 && picker.query.is_empty() {
                picker.scope = ResumeScope::CurrentDir;
            }
            let len = picker.filtered().len();
            if len == 0 {
                picker.selected = 0;
            } else {
                picker.selected = picker.selected.min(len - 1);
            }
        }
    }

    fn open_resume_picker(&mut self) {
        if let Some(client) = &self.client {
            client.send(AcpCommand::ListSessions { cwd: None });
        }
        let items = self.build_resume_items();
        let has_current_dir = items.iter().any(|i| i.same_cwd);
        let scope = if has_current_dir {
            ResumeScope::CurrentDir
        } else {
            ResumeScope::All
        };
        let mut picker = PickerState::new(PickerKind::Resume, "恢复历史会话", items, 0);
        picker.scope = scope;
        picker.current_cwd = self.cwd.clone();
        if let Some(pos) = picker.filtered().iter().position(|(_, item)| item.active) {
            picker.selected = pos;
        }
        self.picker = Some(picker);
    }

    fn open_theme_picker(&mut self) {
        let cur = theme::current_preset();
        let items: Vec<PickerItem> = ThemePreset::ALL
            .iter()
            .map(|&p| PickerItem::simple(p.id(), p.label(), p.id(), p == cur))
            .collect();
        let selected = items.iter().position(|i| i.active).unwrap_or(0);
        self.picker = Some(PickerState::new(
            PickerKind::Theme,
            "选择配色主题",
            items,
            selected,
        ));
    }

    fn open_status_picker(&mut self) {
        use crate::transcript::format_short;
        let used = self.transcript.context_used;
        let win = self.transcript.context_window;
        let pct = if win > 0 {
            (used as f64 / win as f64) * 100.0
        } else {
            0.0
        };
        let denom = self.transcript.total_input + self.transcript.total_cached_read;
        let hit = if denom > 0 {
            (self.transcript.total_cached_read * 100) / denom
        } else {
            0
        };
        let sid = self
            .transcript
            .session_id
            .as_deref()
            .unwrap_or("未连接")
            .to_string();

        let items = vec![
            PickerItem::simple(
                "compact",
                "压缩上下文 (/compact)",
                format!(
                    "当前 {}/{} ({:.1}%) · 回车立即压缩释放上下文",
                    format_short(used),
                    format_short(win),
                    pct
                ),
                true,
            ),
            PickerItem::simple(
                "model",
                format!("切换模型（当前 {}）", self.model_label()),
                format!(
                    "累计 输入 {} · 输出 {} · 缓存命中 {}%",
                    format_short(self.transcript.total_input),
                    format_short(self.transcript.total_output),
                    hit
                ),
                false,
            ),
            PickerItem::simple("copy_id", "复制完整会话 ID", sid, false),
        ];
        self.picker = Some(PickerState::new(
            PickerKind::Status,
            "上下文与会话状态",
            items,
            0,
        ));
    }

    fn jump_to_bottom(&mut self) {
        let max_scroll = self
            .last_hits
            .total_body_rows
            .saturating_sub(self.last_hits.visible_body_rows);
        self.scroll = max_scroll;
        self.auto_follow = true;
    }

    fn scroll_to_scrollbar_row(&mut self, row: u16) {
        let bar = self.last_hits.scrollbar;
        if bar.height == 0 {
            return;
        }
        let max_scroll = self
            .last_hits
            .total_body_rows
            .saturating_sub(self.last_hits.visible_body_rows);
        if max_scroll == 0 {
            self.scroll = 0;
            self.auto_follow = true;
            return;
        }
        let clamped = row.clamp(bar.y, bar.bottom().saturating_sub(1));
        let rel = clamped.saturating_sub(bar.y) as f64;
        let span = bar.height.saturating_sub(1).max(1) as f64;
        let target = ((rel / span) * max_scroll as f64).round() as usize;
        self.scroll = target.min(max_scroll);
        self.auto_follow = self.scroll >= max_scroll;
    }

    fn open_help_picker(&mut self) {
        let rows = [
            ("Enter / Shift+Enter", "发送消息 / 输入换行（亦支持 Ctrl+J、Alt+Enter）"),
            ("Ctrl+A / Ctrl+E", "光标移至输入框最开头 / 最末尾（Home/End 移至当前行首尾）"),
            ("Ctrl+G (Alt+G)", "用 $EDITOR / notepad.exe 打开输入框正文，保存退出后替换"),
            ("Ctrl+V / Alt+V / 拖拽", "粘贴截图/图片或大段文本（自动折叠为原子块，退格一键删除）"),
            ("/image [路径]", "手动粘贴剪贴板图片或附加指定本地图片"),
            ("Shift + Tab", "快速循环切换执行模式 (default / accept-edits / plan)"),
            ("Ctrl + B", "显示 / 隐藏右侧边栏"),
            ("Ctrl + O", "展开 / 折叠全部思考过程与长 Diff / 工具详情"),
            ("PgUp / PgDn / 滚轮", "滚动对话内容；End 回到底部恢复跟随"),
            ("Esc", "中断当前生成 / 关闭浮层 / 清空输入框"),
            ("/model · /effort · /mode", "交互式切换模型、思考等级、执行模式"),
            ("/resume · /new", "按当前目录/全部与最近时间搜索恢复会话（Tab 切换范围）"),
            ("/theme · /bg", "切换 5 套 MD3/TokyoNight 配色 / 开关透明背景"),
            ("/copy [n|all] · /mouse", "复制回答原文 / 开关鼠标捕获；直接左键拖选松手自动复制"),
            ("鼠标拖拽 / 点击", "左键拖选正文或侧边栏松手即复制（不串栏）；点击展开或开链接"),
        ];
        let items = rows
            .iter()
            .map(|(k, v)| PickerItem::simple(*k, format!("{k:<24}"), *v, false))
            .collect();
        self.picker = Some(PickerState::new(
            PickerKind::Help,
            "litemiyu-acp 快捷键与命令帮助",
            items,
            0,
        ));
    }

    fn set_config_option(&mut self, config_id: &str, value: &str) {
        let Some(sid) = self.transcript.session_id.clone() else {
            return;
        };
        if let Some(client) = &self.client {
            client.send(AcpCommand::SetConfigOption {
                session_id: sid,
                config_id: config_id.to_string(),
                value: value.to_string(),
            });
        }
    }

    fn set_mode(&mut self, mode_id: &str) {
        self.current_mode = mode_id.to_string();
        if self
            .config_options
            .iter()
            .any(|c| c.id == "mode" || c.category == "mode")
        {
            self.set_config_option("mode", mode_id);
        } else if let (Some(client), Some(sid)) = (&self.client, self.transcript.session_id.clone())
        {
            client.send(AcpCommand::SetMode {
                session_id: sid,
                mode_id: mode_id.to_string(),
            });
        }
    }

    fn cycle_mode(&mut self) {
        let list: Vec<String> = self
            .config_options
            .iter()
            .find(|c| c.id == "mode" || c.category == "mode")
            .map(|c| c.options.iter().map(|o| o.value.clone()).collect())
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| self.modes.iter().map(|m| m.id.clone()).collect());
        if list.len() < 2 {
            return;
        }
        let pos = list
            .iter()
            .position(|m| m == &self.current_mode)
            .unwrap_or(0);
        let next = list[(pos + 1) % list.len()].clone();
        self.set_mode(&next);
        self.toast(format!("模式切换为：{next}"), false);
    }

    fn start_new_session(&mut self) {
        self.transcript.clear();
        self.pending_images.clear();
        self.image_seq = 0;
        self.busy = false;
        self.connecting = true;
        self.auto_follow = true;
        self.scroll = 0;
        if let Some(client) = &self.client {
            client.send(AcpCommand::NewSession {
                cwd: self.cwd.clone(),
            });
        }
    }

    fn load_session(&mut self, session_id: String) {
        if self.busy {
            self.toast("请先中断或等待当前回复完成再切换会话", true);
            return;
        }
        let target_cwd = self
            .recent_sessions
            .iter()
            .find(|s| s.session_id == session_id && !s.cwd.is_empty())
            .map(|s| s.cwd.clone())
            .unwrap_or_else(|| self.cwd.clone());
        self.transcript.clear();
        self.pending_images.clear();
        self.image_seq = 0;
        self.cwd = target_cwd.clone();
        self.connecting = true;
        self.auto_follow = true;
        self.scroll = 0;
        if let Some(client) = &self.client {
            client.send(AcpCommand::LoadSession {
                session_id,
                cwd: target_cwd,
            });
        }
    }

    fn cancel_turn(&mut self) {
        if let (Some(client), Some(sid)) = (&self.client, self.transcript.session_id.clone()) {
            client.send(AcpCommand::Cancel { session_id: sid });
            self.toast("已发送中断请求…", false);
        }
    }

    fn scroll_up(&mut self, lines: usize) {
        self.auto_follow = false;
        self.scroll = self.scroll.saturating_sub(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        let max_scroll = self
            .last_hits
            .total_body_rows
            .saturating_sub(self.last_hits.visible_body_rows);
        self.scroll = (self.scroll + lines).min(max_scroll);
        if self.scroll >= max_scroll {
            self.auto_follow = true;
        }
    }

    fn save_prefs(&self) {
        let prefs = UserPrefs {
            theme: theme::current_preset().id().to_string(),
            paint_bg: theme::paint_bg(),
            show_sidebar: self.show_sidebar,
        };
        prefs.save();
    }
}

fn is_ctrl_char(code: KeyCode, modifiers: KeyModifiers, target: char) -> bool {
    let lower = target.to_ascii_lowercase();
    let upper = target.to_ascii_uppercase();
    if modifiers == KeyModifiers::CONTROL
        && matches!(code, KeyCode::Char(c) if c == lower || c == upper)
    {
        return true;
    }
    // 兼容部分终端直接发送 ASCII 控制字符（Ctrl+A = 0x01 .. Ctrl+Z = 0x1A）
    if lower.is_ascii_lowercase() {
        let ctrl_ch = ((lower as u8) - b'a' + 1) as char;
        if matches!(code, KeyCode::Char(c) if c == ctrl_ch)
            && !modifiers.contains(KeyModifiers::ALT)
        {
            return true;
        }
    }
    false
}

fn rect_contains(rect: ratatui::layout::Rect, col: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && col >= rect.x
        && col < rect.right()
        && row >= rect.y
        && row < rect.bottom()
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    #[test]
    fn ctrl_a_ctrl_e_and_ctrl_g_shortcuts_work_on_multiline_input() {
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        app.editor.set_text("line1\nline2\nline3");

        // Ctrl+A 回到整个输入框最开头（即便在多行末尾）
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(app.editor.text_before_cursor(), "");

        // Ctrl+E 回到整个输入框最末尾
        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(app.editor.text_before_cursor(), "line1\nline2\nline3");

        // 兼容 ASCII 控制字符形式（0x01 / 0x05 / 0x07）
        app.handle_key(KeyEvent::new(KeyCode::Char('\x01'), KeyModifiers::NONE));
        assert_eq!(app.editor.text_before_cursor(), "");
        app.handle_key(KeyEvent::new(KeyCode::Char('\x05'), KeyModifiers::NONE));
        assert_eq!(app.editor.text_before_cursor(), "line1\nline2\nline3");

        // Ctrl+G 触发外部编辑器挂起标志，回调替换后光标落在末尾
        assert!(!app.external_editor_pending);
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(app.external_editor_pending);
        app.external_editor_pending = false;

        let initial = app.prepare_external_editor_buffer();
        assert_eq!(initial, "line1\nline2\nline3");
        app.apply_external_editor_result(Ok("外部编辑后的新内容\n第二行".into()));
        assert_eq!(app.editor.text(), "外部编辑后的新内容\n第二行");
        assert_eq!(app.editor.text_before_cursor(), "外部编辑后的新内容\n第二行");
    }

    #[test]
    fn shift_enter_and_lf_insert_newline_without_submitting() {
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        let initial_entries = app.transcript.entries.len();
        app.editor.set_text("第一行");

        // 1. Kitty 键盘增强协议上报的 Shift+Enter 插入换行，不提交消息
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(app.editor.text(), "第一行\n");
        assert_eq!(app.transcript.entries.len(), initial_entries);

        // 2. 终端直接发送的裸 LF（\n）同样插入换行，不提交消息
        app.handle_key(KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::NONE));
        assert_eq!(app.editor.text(), "第一行\n\n");
        assert_eq!(app.transcript.entries.len(), initial_entries);

        // 3. 裸修饰键事件被忽略，不清空输入
        app.handle_key(KeyEvent::new(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftShift),
            KeyModifiers::SHIFT,
        ));
        assert_eq!(app.editor.text(), "第一行\n\n");
    }
}
