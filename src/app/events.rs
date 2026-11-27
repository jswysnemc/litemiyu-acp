//! ACP 异步事件与流式 `session/update` 状态分发。

use crate::acp::{AcpCommand, AcpEvent, SessionUpdate};
use crate::transcript::short_id;
use crate::ui::overlay::parse_timestamp_secs;

use super::App;

impl App {
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
                    if let Some(sid) = self.pending_resume.clone() {
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
                // 历史回放完成后重置后台任务与异步续答状态，避免将旧会话记录误判为运行中
                self.pending_resume = None;
                self.background.clear();
                self.todo_call_ids.clear();
                self.async_turn_last_event = None;
                self.async_turn_saw_usage = false;
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
            AcpEvent::Update { session_id, update } => {
                // 忽略已切换会话后来自旧会话的迟到推送，避免串会话污染状态与动效
                let current = self.transcript.session_id.as_deref();
                let stale = if self.connecting {
                    self.pending_resume.as_deref().map_or(current == Some(session_id.as_str()), |target| target != session_id)
                } else {
                    current.is_some_and(|active| active != session_id)
                };
                if !session_id.is_empty() && stale {
                    return;
                }
                self.handle_session_update(update);
            }
            AcpEvent::PermissionRequested(req) => {
                self.permission = Some((req, 0));
            }
            AcpEvent::ElicitationRequested(req) => {
                self.elicitation = Some(super::ElicitationState::new(req));
            }
            AcpEvent::PromptFinished {
                prompt_id,
                stop_reason,
                usage,
            } => {
                self.handle_prompt_finished(prompt_id, stop_reason, usage);
            }
            AcpEvent::PromptRejected { prompt_id, message } => {
                self.handle_prompt_rejected(prompt_id, message);
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
                self.in_flight_prompts.clear();
                self.busy = false;
                self.async_turn_last_event = None;
                self.async_turn_saw_usage = false;
                self.permission = None;
                self.elicitation = None;
                self.background.clear();
                self.subagents.clear();
                self.todo_call_ids.clear();
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
                if !text.is_empty() {
                    self.mark_async_turn_activity();
                }
                self.transcript.append_thought(&text);
            }
            SessionUpdate::AgentMessageChunk(text) => {
                if !text.is_empty() {
                    self.mark_async_turn_activity();
                }
                if !self.connecting {
                    self.background.on_agent_message_chunk(&text);
                }
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
                // 【任务清单】【拦截工具调用】将 agy `update_steps` 及同类 Todo 工具转为结构化任务清单，避免生成空白工具行
                if crate::todo::is_todo_tool(&name) {
                    if !call_id.is_empty() {
                        self.todo_call_ids.insert(call_id);
                    }
                    if let Some(items) =
                        crate::todo::extract_plan_items_from_tool(&name, raw_input.as_ref())
                    {
                        if !items.is_empty() {
                            self.mark_async_turn_activity();
                        }
                        self.transcript.update_plan(items);
                    }
                    return;
                }
                let is_active = !matches!(
                    status.trim().to_ascii_lowercase().as_str(),
                    "completed" | "failed" | "error" | "cancelled" | "canceled" | "done" | "succeeded"
                );
                if is_active {
                    self.mark_async_turn_activity();
                }
                let entry_id = self.transcript.upsert_tool(
                    call_id.clone(),
                    Some(name.clone()),
                    Some(title.clone()),
                    Some(kind.clone()),
                    Some(status.clone()),
                    raw_input.clone(),
                    output,
                    diffs,
                );
                if let Some(snapshot) = self.transcript.tool_snapshot(&call_id) { self.subagents.update(snapshot); }
                if !self.connecting
                    && let Some(done) = self.background.on_tool_event(
                        &call_id,
                        entry_id,
                        Some(&name),
                        Some(&title),
                        Some(&kind),
                        Some(&status),
                        raw_input.as_ref(),
                    )
                    && !self.busy
                {
                    let msg = if done.failed {
                        format!("后台任务失败：{}", done.label)
                    } else {
                        format!("后台任务已完成：{}", done.label)
                    };
                    self.toast(msg, done.failed);
                }
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
                let is_terminal = status.as_deref().is_some_and(|st| {
                    matches!(
                        st.trim().to_ascii_lowercase().as_str(),
                        "completed" | "failed" | "error" | "cancelled" | "canceled" | "done" | "succeeded"
                    )
                });
                // 【任务清单】【拦截增量更新】处理延迟回传 `rawInput` 的 Todo 工具更新事件
                let matched_todo = name
                    .as_deref()
                    .is_some_and(crate::todo::is_todo_tool)
                    || (!call_id.is_empty() && self.todo_call_ids.contains(&call_id));
                if matched_todo {
                    if !call_id.is_empty() {
                        self.todo_call_ids.insert(call_id);
                    }
                    let tool_name = name.as_deref().unwrap_or("update_steps");
                    if let Some(items) =
                        crate::todo::extract_plan_items_from_tool(tool_name, raw_input.as_ref())
                    {
                        if !items.is_empty() {
                            self.mark_async_turn_activity();
                        }
                        self.transcript.update_plan(items);
                    }
                    return;
                }
                if !is_terminal {
                    self.mark_async_turn_activity();
                }
                let entry_id = self.transcript.upsert_tool(
                    call_id.clone(),
                    name.clone(),
                    title.clone(),
                    kind.clone(),
                    status.clone(),
                    raw_input.clone(),
                    output,
                    diffs,
                );
                if let Some(snapshot) = self.transcript.tool_snapshot(&call_id) { self.subagents.update(snapshot); }
                if !self.connecting
                    && let Some(done) = self.background.on_tool_event(
                        &call_id,
                        entry_id,
                        name.as_deref(),
                        title.as_deref(),
                        kind.as_deref(),
                        status.as_deref(),
                        raw_input.as_ref(),
                    )
                    && !self.busy
                {
                    let msg = if done.failed {
                        format!("后台任务失败：{}", done.label)
                    } else {
                        format!("后台任务已完成：{}", done.label)
                    };
                    self.toast(msg, done.failed);
                }
            }
            SessionUpdate::Plan(items) => {
                if !items.is_empty() {
                    self.mark_async_turn_activity();
                }
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
                if self.async_turn_last_event.is_some() {
                    self.async_turn_saw_usage = true;
                }
            }
        }
    }

}
