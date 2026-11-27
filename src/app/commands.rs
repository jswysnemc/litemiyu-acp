//! 用户消息提交、本地斜杠命令、会话新建/恢复与模型模式配置指令。

use crate::acp::AcpCommand;
use crate::clipboard;
use crate::theme;

use crate::ui::overlay::PickerKind;

use super::{App, PendingImage, QueuedMessage, QueuedMode};

impl App {
    pub(crate) fn submit_text(&mut self, text: String) {
        self.submit_message(text, false);
    }

    /// 提交用户输入：本地命令即时生效；普通消息按空闲/生成态与 `queued_mode` 决定直发、排队或并发插话。
    ///
    /// 参数:
    /// - `text`: 输入框展开后的文本
    /// - `force_immediate`: 为 `true` 时无视 `Queue` 模式直接并发发送（如 `Ctrl+S`）
    pub(super) fn submit_message(&mut self, text: String, force_immediate: bool) {
        let trimmed = text.trim();
        let pending = std::mem::take(&mut self.pending_images);
        let attached: Vec<PendingImage> = pending
            .into_iter()
            .filter(|p| text.contains(&p.label) || trimmed.is_empty())
            .collect();

        if trimmed.is_empty() && attached.is_empty() {
            return;
        }
        if attached.is_empty()
            && let Some(rest) = trimmed.strip_prefix('/')
        {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
            let arg = parts.next().unwrap_or("").trim();
            if self.try_local_command(&cmd, arg) {
                return;
            }
        }

        if self.connecting || self.transcript.session_id.is_none() {
            self.pending_images = attached;
            self.toast("ACP 会话尚未就绪，请稍候", true);
            return;
        }

        let msg = QueuedMessage {
            text,
            images: attached,
            already_displayed: false,
        };
        if self.is_generating() {
            if force_immediate || self.queued_mode == QueuedMode::SendImmediately {
                self.dispatch_prompt(msg);
            } else {
                self.enqueue_message(msg);
            }
            return;
        }
        self.dispatch_prompt(msg);
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
            "queue" => {
                let sub = arg.trim();
                if sub.is_empty() {
                    self.open_queue_picker();
                } else if sub.eq_ignore_ascii_case("toggle") {
                    let next = match self.queued_mode {
                        QueuedMode::Queue => QueuedMode::SendImmediately,
                        QueuedMode::SendImmediately => QueuedMode::Queue,
                    };
                    self.set_queued_mode(next);
                } else if sub.eq_ignore_ascii_case("send") {
                    if !self.send_queued_head_immediately() {
                        self.toast("当前待发队列为空", false);
                    }
                } else if sub.eq_ignore_ascii_case("pop") {
                    if !self.recall_last_queued_to_editor() {
                        self.toast("当前待发队列为空", false);
                    }
                } else if sub.eq_ignore_ascii_case("clear") {
                    let cleared = self.clear_queued_messages();
                    self.toast(format!("已清空 {cleared} 条待发队列消息"), false);
                } else if let Some(mode) = QueuedMode::parse(sub) {
                    self.set_queued_mode(mode);
                } else {
                    self.toast(
                        "用法: /queue [queue|steer|toggle|send|pop|clear]",
                        true,
                    );
                }
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
                self.handle_theme_command(arg);
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
            "todo" => {
                self.toggle_todo_bar();
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

    /// 处理 `/theme` 及其子命令（直接切换、热重载、导出自定义模板、查看配置目录或打开选择器）。
    ///
    /// 参数:
    /// - `arg`: `/theme` 后附带的参数字符串
    fn handle_theme_command(&mut self, arg: &str) {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            self.open_theme_picker();
            return;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let sub = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();

        // 1. `/theme reload`：重新扫描用户 themes 目录并实时刷新当前自定义配色
        if sub.eq_ignore_ascii_case("reload") {
            let count = theme::reload_custom_themes();
            self.toast(format!("已重载自定义主题（共 {count} 套）"), false);
            return;
        }
        // 2. `/theme export [name]` 或 `/theme init [name]`：将当前主题导出为 JSON 模板
        if sub.eq_ignore_ascii_case("export") || sub.eq_ignore_ascii_case("init") {
            match theme::export_current_theme(rest) {
                Ok(path) => {
                    let display = path.display().to_string();
                    clipboard::copy_text(&display);
                    self.toast(format!("已导出主题模板（路径已复制）：{display}"), false);
                }
                Err(err) => self.toast(err, true),
            }
            return;
        }
        // 3. `/theme dir` 或 `/theme path`：展示并复制自定义主题目录路径
        if sub.eq_ignore_ascii_case("dir") || sub.eq_ignore_ascii_case("path") {
            if let Some(dir) = crate::paths::themes_dir() {
                let _ = std::fs::create_dir_all(&dir);
                let display = dir.display().to_string();
                clipboard::copy_text(&display);
                self.toast(format!("自定义主题目录（已复制）：{display}"), false);
            } else {
                self.toast("无法解析自定义主题目录", true);
            }
            return;
        }

        // 4. 按内置 ID/别名或自定义 ID 直接切换（先热重载一次以识别新放入的文件）
        let _ = theme::reload_custom_themes();
        if let Some(label) = theme::set_by_id(trimmed) {
            self.save_prefs();
            self.toast(format!("已切换主题：{label}"), false);
        } else {
            self.toast(
                format!("未找到主题「{trimmed}」，已打开选择器"),
                true,
            );
            self.open_theme_picker();
        }
    }


    pub(super) fn set_config_option(&mut self, config_id: &str, value: &str) {
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

    pub(super) fn set_mode(&mut self, mode_id: &str) {
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

    pub(super) fn cycle_mode(&mut self) {
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

    pub(super) fn start_new_session(&mut self) {
        self.pending_resume = None;
        self.transcript.clear();
        self.subagents.clear();
        self.background.clear();
        self.todo_call_ids.clear();
        self.todo_bar_expanded = true;
        self.completion_cache.invalidate();
        self.completion_dismissed_text = None;
        self.pending_images.clear();
        self.image_seq = 0;
        self.queued_messages.clear();
        self.in_flight_prompts.clear();
        self.busy = false;
        self.permission = None;
        self.elicitation = None;
        self.async_turn_last_event = None;
        self.async_turn_saw_usage = false;
        self.connecting = true;
        self.auto_follow = true;
        self.scroll = 0;
        if let Some(client) = &self.client {
            client.send(AcpCommand::NewSession {
                cwd: self.cwd.clone(),
            });
        }
    }

    pub(super) fn load_session(&mut self, session_id: String) {
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
        self.pending_resume = Some(session_id.clone());
        self.transcript.clear();
        self.subagents.clear();
        self.background.clear();
        self.todo_call_ids.clear();
        self.todo_bar_expanded = true;
        self.completion_cache.invalidate();
        self.completion_dismissed_text = None;
        self.permission = None;
        self.elicitation = None;
        self.async_turn_last_event = None;
        self.async_turn_saw_usage = false;
        self.pending_images.clear();
        self.image_seq = 0;
        self.queued_messages.clear();
        self.in_flight_prompts.clear();
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

    /// 切换底部任务清单停靠条的展开/收起状态（无任务清单时弹出轻量提示）。
    pub(super) fn toggle_todo_bar(&mut self) {
        if self.transcript.latest_plan.is_empty() {
            self.toast("当前会话暂无任务清单", false);
            return;
        }
        self.todo_bar_expanded = !self.todo_bar_expanded;
        let counts = crate::todo::count_items(&self.transcript.latest_plan);
        let msg = if self.todo_bar_expanded {
            format!("已展开任务清单 ({}/{})", counts.finished(), counts.total)
        } else {
            "已收起底部任务清单为单行".to_string()
        };
        self.toast(msg, false);
    }

    pub(super) fn cancel_turn(&mut self) {
        if let Some(state) = self.elicitation.take()
            && let Some(client) = &self.client
        {
            client.send(AcpCommand::AnswerElicitation {
                rpc_id: state.req.rpc_id,
                action: "cancel".to_string(),
                content: None,
            });
        }
        if let (Some(client), Some(sid)) = (&self.client, self.transcript.session_id.clone()) {
            client.send(AcpCommand::Cancel { session_id: sid });
            self.toast("已发送中断请求…", false);
        }
        // 若处于无 PromptFinished 响应的异步续答期，中断后立即收尾写入已中断标记
        if !self.busy && self.async_turn_last_event.take().is_some() {
            self.async_turn_saw_usage = false;
            let label = self.turn_done_model_label();
            self.transcript.finish_turn(&label, "cancelled", None);
        }
    }

}
