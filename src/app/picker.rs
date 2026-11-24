//! 权限审批抽屉与模态选择器（模型/思考等级/模式/历史会话/主题/状态/帮助）交互。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::acp::{AcpCommand, SessionSummary};
use crate::clipboard;
use crate::theme::{self, ThemePreset};
use crate::transcript::short_id;
use crate::ui::overlay::{
    PickerItem, PickerKind, PickerState, ResumeScope, format_relative_time, parse_timestamp_secs,
    same_directory,
};

use super::App;

impl App {
    pub(super) fn handle_permission_key(&mut self, key: KeyEvent) {
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
        if let Some((req, _)) = self.permission.take()
            && let Some(client) = &self.client {
                client.send(AcpCommand::AnswerPermission {
                    rpc_id: req.rpc_id,
                    option_id,
                });
            }
    }

    pub(super) fn handle_picker_key(&mut self, key: KeyEvent) {
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


    pub(super) fn open_config_picker(&mut self, config_id: &str, title: &str, kind: PickerKind) {
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

    pub(super) fn open_mode_picker(&mut self) {
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

    pub(super) fn refresh_resume_picker_items(&mut self) {
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

    pub(super) fn open_resume_picker(&mut self) {
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

    pub(super) fn open_theme_picker(&mut self) {
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

    pub(super) fn open_status_picker(&mut self) {
        use crate::transcript::format_short;
        let used = self.transcript.context_used;
        let win = self.transcript.context_window;
        let pct = if win > 0 {
            (used as f64 / win as f64) * 100.0
        } else {
            0.0
        };
        let denom = self.transcript.total_input + self.transcript.total_cached_read;
        let hit = (self.transcript.total_cached_read * 100)
            .checked_div(denom)
            .unwrap_or(0);
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


    pub(super) fn open_help_picker(&mut self) {
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

}
