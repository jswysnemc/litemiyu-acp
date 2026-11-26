//! 运行中用户输入的消息队列与并发插话（steer）调度逻辑。

use serde::{Deserialize, Serialize};

use crate::acp::{AcpCommand, ImagePayload, TurnUsage};
use crate::transcript::ImageChip;
use crate::ui::overlay::{PickerItem, PickerKind, PickerState};

use super::{App, PendingImage};

/// Agent 运行期间新提交消息的处理策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueuedMode {
    /// 先进入本地待发队列，等待当前回合结束后自动发送
    #[default]
    Queue,
    /// 不阻塞等待，直接并发发出 Prompt（由支持 steer 的后端中途融合）
    SendImmediately,
}

impl QueuedMode {
    /// 返回用于选择器展示的完整中文标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Queue => "排队等待 (queue)",
            Self::SendImmediately => "即时插话 (steer)",
        }
    }

    /// 返回用于底栏紧凑徽标的简短中文标签。
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Queue => "排队",
            Self::SendImmediately => "即时插话",
        }
    }

    /// 解析 `/queue` 命令参数。
    ///
    /// 参数:
    /// - `raw`: 用户传入的子参数
    ///
    /// 返回:
    /// - 匹配时返回目标 `QueuedMode`，无法识别返回 `None`
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "queue" | "q" | "wait" | "排队" => Some(Self::Queue),
            "steer" | "immediate" | "send-immediately" | "now" | "插话" | "即时" => {
                Some(Self::SendImmediately)
            }
            _ => None,
        }
    }
}

/// 一条已提交但等待发出或在途并发发送的用户消息。
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub text: String,
    pub images: Vec<PendingImage>,
    /// 并发插话被后端拒绝并回退队列后若已插入记录，不再重复写入一条用户气泡
    pub already_displayed: bool,
}

impl QueuedMessage {
    /// 生成单行队列预览文本（折叠换行并在仅附带图片时展示数量占位）。
    ///
    /// 返回:
    /// - 单行紧凑摘要文本
    pub fn preview(&self) -> String {
        let single_line: String = self
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if single_line.is_empty() && !self.images.is_empty() {
            return format!("[图片 x{}]", self.images.len());
        }
        single_line
    }
}

/// 跟踪单个已发往 ACP 后端但尚未返回响应的 Prompt 请求。
#[derive(Debug, Clone)]
pub struct InFlightPrompt {
    pub id: u64,
    pub message: QueuedMessage,
    pub user_entry_id: Option<u64>,
    pub was_concurrent: bool,
}

impl App {
    /// 组装一条标准待发/插话消息的展示文案与图片元信息。
    fn build_prompt_payloads(msg: &QueuedMessage) -> (String, Vec<ImageChip>, Vec<ImagePayload>) {
        let trimmed = msg.text.trim();
        let display_text = if trimmed.is_empty() {
            msg.images
                .iter()
                .map(|a| a.label.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            msg.text.clone()
        };
        let chips = msg
            .images
            .iter()
            .map(|a| ImageChip {
                label: a.label.clone(),
                path: a
                    .image
                    .path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
            })
            .collect();
        let payloads = msg
            .images
            .iter()
            .map(|a| ImagePayload {
                mime_type: a.image.mime_type.clone(),
                base64_data: a.image.base64_data.clone(),
            })
            .collect();
        (display_text, chips, payloads)
    }

    /// 立即向 ACP 后端分发一条消息（支持空闲直发与生成期并发插话）。
    ///
    /// 参数:
    /// - `msg`: 待发送的消息载荷（含文本与附加图片）
    pub(super) fn dispatch_prompt(&mut self, msg: QueuedMessage) {
        let Some(sid) = self.transcript.session_id.clone() else {
            self.queued_messages.push_front(msg);
            self.toast("ACP 会话尚未就绪，消息已暂存队列", true);
            return;
        };

        // 1. 构建用户可见文本、图片标签与协议二进制载荷，并透明转译 #skill 引用
        let (display_text, chips, payloads) = Self::build_prompt_payloads(&msg);
        let prompt_text = if msg.text.contains('#') {
            let skills = self.completion_cache.skills_for(&self.cwd);
            crate::completion::rewrite_skill_mentions(&msg.text, skills, &self.available_commands)
        } else {
            msg.text.clone()
        };

        // 2. 非并发新回合前闭合遗留异步续答并重置回合计时；并发插话保持现有回合计时
        let was_concurrent = !self.in_flight_prompts.is_empty();
        if !was_concurrent {
            self.finish_async_turn();
            self.transcript.begin_turn();
        }

        // 3. 写入对话用户消息记录（拒绝回滚保留条目时不重复追加）
        let user_entry_id = if msg.already_displayed {
            None
        } else {
            self.transcript.push_user(display_text, chips);
            self.transcript.entries.last().map(|e| e.id)
        };

        self.auto_follow = true;
        if let Some(client) = &self.client {
            // 4. 登记在途 Prompt 编号并发送到 ACP 后端
            let prompt_id = self.next_prompt_id;
            self.next_prompt_id = self.next_prompt_id.wrapping_add(1).max(1);
            self.in_flight_prompts.push(InFlightPrompt {
                id: prompt_id,
                message: msg,
                user_entry_id,
                was_concurrent,
            });
            self.busy = true;
            client.send(AcpCommand::Prompt {
                prompt_id,
                session_id: sid,
                text: prompt_text,
                images: payloads,
            });
            if was_concurrent {
                self.toast("已并发插话发送，等待 Agent 融合处理", false);
            }
        } else {
            self.busy = false;
            self.transcript
                .push_notice("当前为 --demo 离线预览模式，去掉 --demo 即可连接 agy acp", false);
        }
    }

    /// 将当前消息加入本地待发队列尾部并给出提示。
    ///
    /// 参数:
    /// - `msg`: 待排队的消息
    pub(super) fn enqueue_message(&mut self, msg: QueuedMessage) {
        self.queued_messages.push_back(msg);
        let count = self.queued_messages.len();
        self.toast(
            format!("已加入待发队列（第 {count} 条 · Ctrl+S 立即插话 · ↑ 撤回）"),
            false,
        );
    }

    /// 当处于纯空闲且无异步续答时，自动出队发送下一条排队消息。
    pub(super) fn flush_next_queued_message(&mut self) {
        if self.busy || self.async_turn_last_event.is_some() || self.transcript.session_id.is_none()
        {
            return;
        }
        if let Some(next) = self.queued_messages.pop_front() {
            self.dispatch_prompt(next);
        }
    }

    /// 立即出队发送队首消息（运行期按并发插话发出，空闲期直接发送）。
    ///
    /// 返回:
    /// - 队列非空并成功触发发送返回 `true`，队列为空返回 `false`
    pub(super) fn send_queued_head_immediately(&mut self) -> bool {
        let Some(next) = self.queued_messages.pop_front() else {
            return false;
        };
        self.dispatch_prompt(next);
        true
    }

    /// 将最近加入队列的一条消息撤回至输入框，便于继续修改。
    ///
    /// 返回:
    /// - 成功撤回返回 `true`，队列为空返回 `false`
    pub(super) fn recall_last_queued_to_editor(&mut self) -> bool {
        let Some(last) = self.queued_messages.pop_back() else {
            return false;
        };
        let markers: Vec<String> = last.images.iter().map(|img| img.label.clone()).collect();
        let restored = if last.text.trim().is_empty() && !markers.is_empty() {
            markers.join(" ")
        } else {
            last.text
        };
        self.editor.replace_from_external(restored, &markers);
        self.pending_images = last.images;
        self.toast("已将排队消息撤回至输入框", false);
        true
    }

    /// 清空当前全部待发送排队消息。
    ///
    /// 返回:
    /// - 清理的消息条数
    pub(super) fn clear_queued_messages(&mut self) -> usize {
        let count = self.queued_messages.len();
        self.queued_messages.clear();
        count
    }

    /// 切换运行中默认发送模式并持久化到偏好设置。
    ///
    /// 参数:
    /// - `mode`: 目标队列/插话模式
    pub(super) fn set_queued_mode(&mut self, mode: QueuedMode) {
        self.queued_mode = mode;
        self.save_prefs();
        self.toast(
            format!("运行中发送模式已切换为：{}", mode.label()),
            false,
        );
    }

    /// 处理某个 `session/prompt` 正常结束响应，仅在全部在途请求收尾后闭合回合与自动出队。
    pub(super) fn handle_prompt_finished(
        &mut self,
        prompt_id: u64,
        stop_reason: String,
        usage: Option<TurnUsage>,
    ) {
        // 1. 移除已结束的在途 Prompt（兼容单元测试传入 prompt_id=0 的情况）
        if let Some(pos) = self.in_flight_prompts.iter().position(|p| p.id == prompt_id) {
            self.in_flight_prompts.remove(pos);
        } else if prompt_id == 0 && !self.in_flight_prompts.is_empty() {
            self.in_flight_prompts.remove(0);
        }
        self.busy = !self.in_flight_prompts.is_empty();

        // 2. 仍有其他并发插话/主回合在途时，暂不闭合 ▣ 结束行也不自动出队
        if self.busy {
            return;
        }

        // 3. 所有在途请求完结，闭合当前回合记录并视终止原因决定是否自动发送队列下一条
        self.async_turn_last_event = None;
        self.async_turn_saw_usage = false;
        self.permission = None;
        self.elicitation = None;
        self.background.on_prompt_finished(&stop_reason);
        let label = self.turn_done_model_label();
        self.transcript
            .finish_turn(&label, &stop_reason, usage.as_ref());

        if stop_reason == "cancelled" {
            let remaining = self.queued_messages.len();
            if remaining > 0 {
                self.toast(
                    format!(
                        "已中断生成（队列保留 {remaining} 条 · ↑ 撤回 · Ctrl+S 发送 · /queue clear 清空）"
                    ),
                    false,
                );
            }
        } else {
            self.flush_next_queued_message();
        }
    }

    /// 处理单个 `session/prompt` 返回 RPC 错误（并发插话被拒绝时无损转入待发队列）。
    pub(super) fn handle_prompt_rejected(&mut self, prompt_id: u64, message: String) {
        let removed = self
            .in_flight_prompts
            .iter()
            .position(|p| p.id == prompt_id)
            .map(|pos| self.in_flight_prompts.remove(pos));
        self.busy = !self.in_flight_prompts.is_empty();

        // 1. 若属于运行中的并发插话被后端拒绝，撤回刚写入的末尾用户气泡并放回队列头部
        if let Some(mut entry) = removed
            && (entry.was_concurrent || self.busy)
        {
            let popped_trailing = entry.user_entry_id.is_some_and(|uid| {
                if self.transcript.entries.last().is_some_and(|e| e.id == uid) {
                    self.transcript.entries.pop();
                    true
                } else {
                    false
                }
            });
            if !popped_trailing && entry.user_entry_id.is_some() {
                entry.message.already_displayed = true;
            }
            self.queued_messages.push_front(entry.message);
            self.toast(
                format!("当前后端拒绝并发插话，已转入待发队列：{message}"),
                true,
            );
            return;
        }

        // 2. 独立请求失败时按错误收尾并提示用户
        self.async_turn_last_event = None;
        self.async_turn_saw_usage = false;
        self.permission = None;
        self.elicitation = None;
        self.background.on_prompt_finished("error");
        let label = self.turn_done_model_label();
        self.transcript.finish_turn(&label, "error", None);
        self.transcript.push_notice(message.clone(), true);
        self.toast(message, true);
    }

    /// 打开 `/queue` 交互选择器，便于切换排队/插话模式及管理当前队列。
    pub(super) fn open_queue_picker(&mut self) {
        let mut items = vec![
            PickerItem::simple(
                "mode:queue",
                "排队等待 (queue)",
                "Agent 运行期间按 Enter 将消息加入待发队列，回合完成后自动发出",
                self.queued_mode == QueuedMode::Queue,
            ),
            PickerItem::simple(
                "mode:steer",
                "即时插话 (steer)",
                "Agent 运行期间按 Enter 直接并发发出消息（需后端支持并发 Prompt）",
                self.queued_mode == QueuedMode::SendImmediately,
            ),
        ];

        if let Some(head) = self.queued_messages.front() {
            let count = self.queued_messages.len();
            items.push(PickerItem::simple(
                "send_next",
                "立即插话发送队首消息 (Ctrl+S)",
                head.preview(),
                false,
            ));
            items.push(PickerItem::simple(
                "recall_last",
                "撤回最近排队消息到输入框 (↑)",
                self.queued_messages
                    .back()
                    .map(QueuedMessage::preview)
                    .unwrap_or_default(),
                false,
            ));
            items.push(PickerItem::simple(
                "clear",
                format!("清空待发队列（共 {count} 条）"),
                "丢弃当前所有尚未发出的排队消息",
                false,
            ));
        }

        self.picker = Some(PickerState::new(
            PickerKind::Queue,
            "运行中消息发送与队列管理 (/queue)",
            items,
            usize::from(self.queued_mode == QueuedMode::SendImmediately),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::{AcpClient, AcpEvent};
    use crate::clipboard::LoadedImage;

    fn make_test_image(label: &str) -> PendingImage {
        PendingImage {
            label: label.to_string(),
            image: LoadedImage {
                mime_type: "image/png".to_string(),
                base64_data: "AQID".to_string(),
                byte_len: 3,
                path: None,
            },
        }
    }

    #[test]
    fn queue_mode_enqueues_while_busy_and_flushes_after_turn_finishes() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(Some(AcpClient::dummy_for_test(tx)), "/tmp".into(), None, false, false);
        app.connecting = false;
        app.transcript.session_id = Some("sess-1".into());
        app.queued_mode = QueuedMode::Queue;

        app.submit_text("第一条请求".into());
        assert!(app.busy);
        assert_eq!(app.in_flight_prompts.len(), 1);
        let first_pid = match rx.try_recv().unwrap() {
            AcpCommand::Prompt { prompt_id, text, .. } => {
                assert_eq!(text, "第一条请求");
                prompt_id
            }
            other => panic!("unexpected cmd: {other:?}"),
        };

        // 运行中按 Queue 模式提交第二条，应进入待发队列而非立即发往后端
        app.submit_text("第二条排队".into());
        assert!(rx.try_recv().is_err());
        assert_eq!(app.queued_messages.len(), 1);
        assert_eq!(app.in_flight_prompts.len(), 1);

        // 第一轮结束后自动出队发送第二条
        app.handle_acp_event(AcpEvent::PromptFinished {
            prompt_id: first_pid,
            stop_reason: "end_turn".into(),
            usage: None,
        });
        assert!(app.busy);
        assert!(app.queued_messages.is_empty());
        assert_eq!(app.in_flight_prompts.len(), 1);
        match rx.try_recv().unwrap() {
            AcpCommand::Prompt { text, .. } => assert_eq!(text, "第二条排队"),
            other => panic!("unexpected cmd: {other:?}"),
        }
    }

    #[test]
    fn steer_mode_and_rejection_rollback_restore_to_front_of_queue() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(Some(AcpClient::dummy_for_test(tx)), "/tmp".into(), None, false, false);
        app.connecting = false;
        app.transcript.session_id = Some("sess-1".into());
        app.queued_mode = QueuedMode::SendImmediately;

        app.submit_text("主回合".into());
        let p1 = match rx.try_recv().unwrap() {
            AcpCommand::Prompt { prompt_id, .. } => prompt_id,
            _ => panic!("expected first prompt"),
        };

        // 即时插话直接并发发送第二条，保留图片附件载荷
        app.pending_images.push(make_test_image("[图片 1]"));
        app.submit_text("并发插话 [图片 1]".into());
        assert_eq!(app.in_flight_prompts.len(), 2);
        let p2 = match rx.try_recv().unwrap() {
            AcpCommand::Prompt { prompt_id, images, .. } => {
                assert_eq!(images.len(), 1);
                prompt_id
            }
            _ => panic!("expected second prompt"),
        };

        // 若后端拒绝并发插话，应撤回末尾用户气泡并无损转入待发队列首部，同时保持主回合 busy
        let entries_before_reject = app.transcript.entries.len();
        app.handle_acp_event(AcpEvent::PromptRejected {
            prompt_id: p2,
            message: "session busy".into(),
        });
        assert!(app.busy);
        assert_eq!(app.in_flight_prompts.len(), 1);
        assert_eq!(app.transcript.entries.len(), entries_before_reject - 1);
        assert_eq!(app.queued_messages.len(), 1);
        assert_eq!(app.queued_messages[0].images.len(), 1);

        // 主回合结束后自动重新发出刚才被拒绝转入队列的插话
        app.handle_acp_event(AcpEvent::PromptFinished {
            prompt_id: p1,
            stop_reason: "end_turn".into(),
            usage: None,
        });
        assert!(app.busy);
        assert!(app.queued_messages.is_empty());
        match rx.try_recv().unwrap() {
            AcpCommand::Prompt { text, images, .. } => {
                assert_eq!(text, "并发插话 [图片 1]");
                assert_eq!(images.len(), 1);
            }
            _ => panic!("expected flushed prompt"),
        }
    }

    #[test]
    fn cancelled_turn_preserves_queue_and_allows_recall_to_editor() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(Some(AcpClient::dummy_for_test(tx)), "/tmp".into(), None, false, false);
        app.connecting = false;
        app.transcript.session_id = Some("sess-1".into());
        app.queued_mode = QueuedMode::Queue;

        app.submit_text("运行中任务".into());
        let p1 = match rx.try_recv().unwrap() {
            AcpCommand::Prompt { prompt_id, .. } => prompt_id,
            _ => panic!("expected prompt"),
        };
        app.submit_text("待编辑后续".into());

        // 中断后不应自动发送队列消息
        app.handle_acp_event(AcpEvent::PromptFinished {
            prompt_id: p1,
            stop_reason: "cancelled".into(),
            usage: None,
        });
        assert!(!app.busy);
        assert!(rx.try_recv().is_err());
        assert_eq!(app.queued_messages.len(), 1);

        // 撤回队列消息回到输入框
        assert!(app.recall_last_queued_to_editor());
        assert!(app.queued_messages.is_empty());
        assert_eq!(app.editor.text(), "待编辑后续");
    }

    #[test]
    fn ctrl_s_steers_current_input_or_flushes_queued_head_immediately() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(Some(AcpClient::dummy_for_test(tx)), "/tmp".into(), None, false, false);
        app.connecting = false;
        app.transcript.session_id = Some("sess-1".into());
        app.queued_mode = QueuedMode::Queue;

        // 1. 发起主回合
        app.submit_text("主任务".into());
        let _p1 = rx.try_recv().unwrap();

        // 2. 先按 Queue 模式排队一条
        app.submit_text("队列第一条".into());
        assert_eq!(app.queued_messages.len(), 1);
        assert!(rx.try_recv().is_err());

        // 3. 输入新内容并按 Ctrl+S，应直接并发插话发出，且不影响原待发队列
        app.editor.insert_str("紧急插话");
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(app.in_flight_prompts.len(), 2);
        assert_eq!(app.queued_messages.len(), 1);
        match rx.try_recv().unwrap() {
            AcpCommand::Prompt { text, .. } => assert_eq!(text, "紧急插话"),
            other => panic!("expected steered prompt: {other:?}"),
        }

        // 4. 输入框为空时再按 Ctrl+S，应立即插话发出待发队列首条
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.queued_messages.is_empty());
        assert_eq!(app.in_flight_prompts.len(), 3);
        match rx.try_recv().unwrap() {
            AcpCommand::Prompt { text, .. } => assert_eq!(text, "队列第一条"),
            other => panic!("expected flushed head prompt: {other:?}"),
        }
    }
}
