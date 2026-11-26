//! 关联 ACP 启动调用与子会话；仅本地 agy 后端可补充日志摘要。

mod log;
mod metadata;
mod parent;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::transcript::{ToolSnapshot, format_duration, short_id};
use log::Cursor;
use metadata::{Launch, child_ids, field};
use parent::ParentLog;

/// 根据 `name` 判断该工具是否启动子代理，消息发送和管理操作不新建记录。
pub fn is_subagent_family_tool(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "invoke_subagent" | "swarm" | "run_workflow"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Launching,
    Launched,
    Running,
    Completed,
    Failed,
}

impl Phase {
    /// 返回当前阶段的中文标签；仅启动成功时不推断子任务已经结束。
    fn label(self) -> &'static str {
        match self {
            Self::Launching => "启动中",
            Self::Launched => "已启动",
            Self::Running => "运行中",
            Self::Completed => "已完成",
            Self::Failed => "失败",
        }
    }

    /// 返回当前阶段是否已有明确的结束信号。
    fn finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

pub struct Agent {
    pub entry_id: u64,
    pub phase: Phase,
    tool: String,
    launch: Launch,
    pub id: Option<String>,
    started: Instant,
    last_activity: Instant,
    cursor: Cursor,
    observed_model: String,
    reported_to_parent: bool,
    current_tool: String,
    reply: String,
    first_stamp: Option<chrono::DateTime<chrono::FixedOffset>>,
    observed_duration: Duration,
    last_step: Option<u64>,
    tokens: u64,
}

impl Agent {
    /// 用已合并的 ACP 工具快照 `tool` 初始化单个子任务记录。
    fn new(tool: &ToolSnapshot<'_>, launch: Launch, id: Option<String>) -> Self {
        Self {
            entry_id: tool.entry_id,
            tool: tool.name.into(),
            launch,
            id,
            phase: Phase::Launching,
            started: Instant::now(),
            last_activity: Instant::now(),
            cursor: Cursor::default(),
            observed_model: String::new(),
            reported_to_parent: false,
            current_tool: String::new(),
            tokens: 0,
            reply: String::new(),
            first_stamp: None,
            observed_duration: Duration::ZERO,
            last_step: None,
        }
    }

    /// 返回角色与阶段的单行摘要，供侧栏和窄屏正文共用。
    pub fn headline(&self) -> String {
        let role = [&self.launch.role, &self.launch.kind, &self.tool]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or(&self.tool);
        format!("{} · {role}", self.phase.label())
    }

    /// 返回可确认的模型、会话、工具及用量，不展示日志步骤或思考正文。
    pub fn details(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let model = if self.observed_model.is_empty() {
            &self.launch.model
        } else {
            &self.observed_model
        };
        if !model.is_empty() {
            lines.push(format!("模型：{model}"));
        }
        if !self.launch.kind.is_empty() {
            lines.push(format!("类型：{}", self.launch.kind));
        }
        if let Some(id) = &self.id {
            lines.push(format!("会话：#{}", short_id(id)));
        }
        if !self.launch.task.is_empty() {
            lines.push(format!("任务：{}", self.launch.task));
        }
        if !self.current_tool.is_empty() && self.phase == Phase::Running {
            lines.push(format!("当前工具：{}", self.current_tool));
        }
        if self.tokens > 0 {
            lines.push(format!("累计输出：{} token", self.tokens));
        }
        if !self.reply.is_empty() {
            lines.push(format!("回复：{}", self.reply));
        }
        lines
    }

    /// 返回观察到的运行时间；未收到运行证据时不显示估算耗时。
    pub fn elapsed_label(&self) -> String {
        if self.phase.finished() && self.first_stamp.is_some() {
            format_duration(self.observed_duration)
        } else if self.phase == Phase::Running {
            format_duration(if self.first_stamp.is_some() {
                self.observed_duration + self.last_activity.elapsed()
            } else {
                self.started.elapsed()
            })
        } else {
            String::new()
        }
    }

    /// 归纳 `row` 的模型、工具与终态；`parent_id` 用于区分向父会话和其他代理发送的消息。
    fn observe(&mut self, row: &Value, parent_id: &str) {
        let kind = row["type"].as_str().unwrap_or("");
        if self.phase.finished() {
            return;
        }
        // 1. 日志重写或回放时按步骤编号去重，耗时取日志时间而非打开历史会话的时间
        if let Some(step) = row["step_index"].as_u64() {
            if self.last_step.is_some_and(|previous| step <= previous) {
                return;
            }
            self.last_step = Some(step);
        }
        if let Some(stamp) = row["created_at"]
            .as_str()
            .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        {
            let first = *self.first_stamp.get_or_insert(stamp);
            self.observed_duration = (stamp - first).to_std().unwrap_or_default();
        }
        if matches!(kind, "SESSION_FAILED" | "CONVERSATION_FAILED" | "CANCELLED") {
            self.phase = Phase::Failed;
            return;
        }
        if kind != "PLANNER_RESPONSE" {
            return;
        }
        self.phase = Phase::Running;
        // 2. 仅摘录明确发往父会话的回复，不展示思考、参数清单或逐步日志
        self.last_activity = Instant::now();
        if let Some(model) = row["model"].as_str() {
            self.observed_model = model.into();
        }
        self.tokens = self
            .tokens
            .saturating_add(row["output_tokens"].as_u64().unwrap_or(0));
        self.current_tool.clear();
        if let Some(calls) = row["tool_calls"]
            .as_array()
            .filter(|calls| !calls.is_empty())
        {
            for call in calls {
                let name = call["name"].as_str().unwrap_or("");
                self.current_tool = name.to_string();
                if name == "send_message"
                    && field(&call["args"], &["Recipient", "recipient"]) == parent_id
                {
                    self.reported_to_parent = true;
                    self.reply = field(&call["args"], &["Message", "message"])
                        .chars()
                        .take(160)
                        .collect();
                }
            }
        } else if self.reported_to_parent
            && row["status"] == "DONE"
            && row["error"].is_null()
            && row["content"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty())
        {
            self.phase = Phase::Completed;
        }
    }
}

#[derive(Default)]
pub struct Tracker {
    root: Option<PathBuf>,
    parent: ParentLog,
    pub agents: Vec<Agent>,
    last_poll: Option<Instant>,
    next_agent: usize,
}

impl Tracker {
    /// 仅为明确启动的本地 agy 设置 `root`；解析失败时保留纯协议显示。
    pub fn enable_local(&mut self, root: PathBuf) {
        self.root = root.canonicalize().ok();
    }

    /// 会话切换或断连时清除展示记录与所有日志偏移，但保留后端设置。
    pub fn clear(&mut self) {
        *self = Self {
            root: self.root.take(),
            ..Self::default()
        };
    }

    /// 【子代理】【接收启动事件】合并 `tool` 的更新；完成启动调用只代表已启动。
    pub fn update(&mut self, tool: ToolSnapshot<'_>) {
        if !is_subagent_family_tool(tool.name) {
            return;
        }
        let mut launch = Launch::from_args(tool.raw_input.unwrap_or(&Value::Null));
        if launch.role.is_empty() && launch.kind.is_empty() && tool.title != tool.name {
            launch.role = tool.title.into();
        }
        let ids = child_ids(tool.output);
        // 1. 将临时启动记录绑定到返回的会话，多代理调用允许一个条目关联多个会话
        if !self
            .agents
            .iter()
            .any(|agent| agent.entry_id == tool.entry_id)
        {
            self.agents.push(Agent::new(&tool, launch.clone(), None));
        }
        for id in ids {
            if self
                .agents
                .iter()
                .any(|agent| agent.entry_id == tool.entry_id && agent.id.as_ref() == Some(&id))
            {
                continue;
            }
            if let Some(agent) = self
                .agents
                .iter_mut()
                .find(|agent| agent.entry_id == tool.entry_id && agent.id.is_none())
            {
                agent.id = Some(id);
            } else {
                self.agents
                    .push(Agent::new(&tool, launch.clone(), Some(id)));
            }
        }
        // 2. 不用启动工具的迟到通知覆盖子日志已经确认的终态
        for agent in self
            .agents
            .iter_mut()
            .filter(|agent| agent.entry_id == tool.entry_id)
        {
            agent.launch = launch.clone();
            if matches!(agent.phase, Phase::Launching | Phase::Launched) {
                agent.phase = match tool.status {
                    "failed" | "error" | "cancelled" | "canceled" => Phase::Failed,
                    "completed" | "succeeded" | "done" => Phase::Launched,
                    _ => agent.phase,
                };
            }
        }
    }

    /// 返回是否还有需要观察的本地日志；缺失的日志不会永久唤醒终端。
    pub fn watching(&self) -> bool {
        self.root.is_some() && self.agents.iter().any(Self::watch_agent)
    }

    /// 返回 `agent` 是否需要轮询；已知正在运行的任务不受启动等待时限影响。
    fn watch_agent(agent: &Agent) -> bool {
        !agent.phase.finished()
            && (agent.phase == Phase::Running
                || agent.last_activity.elapsed() < Duration::from_secs(90))
    }

    /// 【子代理】【刷新摘要】按 250 毫秒节拍观察 `parent_id` 及其已识别的子日志，返回是否需要重绘。
    pub fn poll(&mut self, parent_id: Option<&str>) -> bool {
        let Some((root, parent_id)) = self.root.as_deref().zip(parent_id) else {
            return false;
        };
        if self.agents.is_empty()
            || self
                .last_poll
                .is_some_and(|last| last.elapsed() < Duration::from_millis(250))
        {
            return false;
        }
        self.last_poll = Some(Instant::now());
        let mut changed = false;
        // 1. ACP 回放可能只有入参；仅在能唯一对应启动请求时才从父日志补齐标识
        if self
            .agents
            .iter()
            .any(|agent| agent.id.is_none() && Self::watch_agent(agent))
        {
            changed |= self.parent.reconcile(root, parent_id, &mut self.agents);
        }
        // 2. 每个节拍最多读取八个子日志，避免大量并发任务阻塞输入
        let count = self.agents.len();
        for _ in 0..count.min(8) {
            let agent = &mut self.agents[self.next_agent % count];
            self.next_agent = (self.next_agent + 1) % count;
            if !Self::watch_agent(agent) {
                continue;
            }
            if let Some(id) = &agent.id
                && id != parent_id
            {
                for record in agent.cursor.read(root, id) {
                    agent.observe(&record, parent_id);
                    changed = true;
                }
            }
        }
        changed
    }
}
