//! 后台任务追踪器（跨 ACP 通用）：
//! 通过标准 ACP `tool_call` / `tool_call_update` 生命周期与回合边界识别仍在后台运行的异步任务，
//! 在不依赖特定后端私有日志路径的前提下，向前台提供准确的运行标记、摘要与跳转目标。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::Value;

/// 单条正在运行的后台任务展示快照。
#[derive(Debug, Clone)]
pub struct BackgroundTask {
    /// ACP 工具调用唯一标识
    pub call_id: String,
    /// 对应正文时间线条目 ID（用于侧边栏点击定位）
    pub entry_id: u64,
    /// 工具名称（如 `run_command`、`invoke_subagent`）
    pub name: String,
    /// 单行紧凑展示标题
    pub label: String,
    /// 工具类别（如 `execute`、`other`）
    pub kind: String,
    /// 任务开始时刻
    pub started_at: Instant,
    /// 是否已确认为后台运行任务
    pub is_background: bool,
}

impl BackgroundTask {
    /// 返回任务已运行的时长。
    ///
    /// 返回:
    /// - 自工具调用启动至今的 `Duration`
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// 后台任务结束时的轻量通知负载。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedBackgroundTask {
    /// 已结束任务的紧凑标题
    pub label: String,
    /// 是否为失败或错误终态
    pub failed: bool,
}

/// 当前会话的活跃工具与后台任务追踪器。
#[derive(Debug, Default)]
pub struct BackgroundTracker {
    active: Vec<BackgroundTask>,
    /// 交互式提问类工具 call_id 集合，确保增量 update 缺省 name 时也不会误入后台追踪
    ignored_calls: HashSet<String>,
}

impl BackgroundTracker {
    /// 清空全部追踪状态（用于新建会话、切换/加载会话或连接断开）。
    pub fn clear(&mut self) {
        self.active.clear();
        self.ignored_calls.clear();
    }

    /// 记录或更新一条实时 `tool_call` / `tool_call_update` 事件。
    ///
    /// 参数:
    /// - `call_id`: ACP 工具调用 ID
    /// - `entry_id`: 对应正文条目 ID
    /// - `name`: 工具名称（增量更新时可能为空）
    /// - `title`: 工具标题（增量更新时可能为空）
    /// - `kind`: 工具类别（增量更新时可能为空）
    /// - `status`: 最新状态字符串（如 `in_progress`、`completed`、`failed`）
    /// - `raw_input`: 工具入参（用于检测显式后台参数与 `toolSummary`）
    ///
    /// 返回:
    /// - 当一条已确认的后台任务到达终态时返回 `Some(FinishedBackgroundTask)`，否则返回 `None`
    #[allow(clippy::too_many_arguments)]
    pub fn on_tool_event(
        &mut self,
        call_id: &str,
        entry_id: u64,
        name: Option<&str>,
        title: Option<&str>,
        kind: Option<&str>,
        status: Option<&str>,
        raw_input: Option<&Value>,
    ) -> Option<FinishedBackgroundTask> {
        if call_id.is_empty() {
            return None;
        }

        // 1. 交互式提问工具与子代理/多代理工具由各自专属模块处理，不纳入通用后台命令追踪
        let raw_name = name.unwrap_or("");
        if is_interactive_question_tool(raw_name)
            || crate::subagent::is_subagent_family_tool(raw_name)
        {
            self.ignored_calls.insert(call_id.to_string());
            if let Some(pos) = self.active.iter().position(|t| t.call_id == call_id) {
                self.active.remove(pos);
            }
            return None;
        }
        if self.ignored_calls.contains(call_id) {
            if let Some(st) = status
                && is_terminal_status(st)
            {
                self.ignored_calls.remove(call_id);
            }
            return None;
        }

        // 2. 若收到终态状态，立即移出活跃列表，并在其为后台任务时返回完成通知
        if let Some(st) = status
            && is_terminal_status(st)
        {
            if let Some(pos) = self.active.iter().position(|t| t.call_id == call_id) {
                let removed = self.active.remove(pos);
                if removed.is_background {
                    return Some(FinishedBackgroundTask {
                        label: removed.label,
                        failed: is_failure_status(st),
                    });
                }
            }
            return None;
        }

        // 2. 更新已有活跃记录，或插入新的活跃工具调用
        let explicit_bg = is_explicit_background(name, raw_input);
        let computed_label = build_compact_label(title, name, raw_input);
        if let Some(existing) = self.active.iter_mut().find(|t| t.call_id == call_id) {
            existing.entry_id = entry_id;
            if let Some(n) = name
                && !n.is_empty()
            {
                existing.name = n.to_string();
            }
            if let Some(k) = kind
                && !k.is_empty()
            {
                existing.kind = k.to_string();
            }
            if let Some(lbl) = computed_label {
                existing.label = lbl;
            }
            if explicit_bg {
                existing.is_background = true;
            }
            return None;
        }

        let name_owned = name.unwrap_or("tool").to_string();
        let label = computed_label.unwrap_or_else(|| name_owned.clone());
        self.active.push(BackgroundTask {
            call_id: call_id.to_string(),
            entry_id,
            name: name_owned,
            label,
            kind: kind.unwrap_or("other").to_string(),
            started_at: Instant::now(),
            is_background: explicit_bg,
        });
        None
    }

    /// 当 Agent 在工具尚未完结时已开始输出正文回复，将仍在运行的命令/异步工具升级为后台任务。
    ///
    /// 参数:
    /// - `chunk`: 收到的助手正文片段
    pub fn on_agent_message_chunk(&mut self, chunk: &str) {
        if chunk.trim().is_empty() {
            return;
        }
        // 1. 工具未完结却已产生正文输出，说明执行类工具已让出控制权并转入后台运行
        for task in &mut self.active {
            if !task.is_background && is_backgroundable_tool(&task.kind, &task.name) {
                task.is_background = true;
            }
        }
    }

    /// 在当前回合 `session/prompt` 结束时处理仍未完结的工具调用。
    ///
    /// 参数:
    /// - `stop_reason`: 本轮结束原因（如 `end_turn`、`cancelled`、`error`）
    pub fn on_prompt_finished(&mut self, stop_reason: &str) {
        // 1. 用户中断回合时，移除仅在当前回合启动的非后台工具，保留已确认的后台任务
        if stop_reason == "cancelled" {
            self.active.retain(|t| t.is_background);
            return;
        }
        // 2. 正常回合结束后仍未收到终态的工具调用，均确认在后台持续运行
        for task in &mut self.active {
            task.is_background = true;
        }
    }

    /// 返回当前已确认在后台运行中的任务列表（按启动先后排序）。
    ///
    /// 返回:
    /// - 正在后台运行的 `BackgroundTask` 切片视图（按顺序收集）
    pub fn running_tasks(&self) -> Vec<BackgroundTask> {
        self.active
            .iter()
            .filter(|t| t.is_background)
            .cloned()
            .collect()
    }

    /// 返回当前已确认在后台运行的任务数量。
    ///
    /// 返回:
    /// - 正在后台运行的任务条数
    #[cfg(test)]
    pub fn running_count(&self) -> usize {
        self.active.iter().filter(|t| t.is_background).count()
    }

    /// 判断是否存在正在后台运行的任务。
    ///
    /// 返回:
    /// - 至少有 1 个后台任务运行时返回 `true`
    pub fn has_running(&self) -> bool {
        self.active.iter().any(|t| t.is_background)
    }

    /// 判断是否存在尚未转入后台的前台进行中工具调用（用于避免异步续答在工具执行期间提前收尾）。
    ///
    /// 返回:
    /// - 存在未完结的前台工具时返回 `true`
    pub fn has_foreground_tool(&self) -> bool {
        self.active.iter().any(|t| !t.is_background)
    }
}

/// 判断工具状态是否属于终态。
fn is_terminal_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "completed" | "failed" | "error" | "cancelled" | "canceled" | "done" | "succeeded"
    )
}

/// 判断终态字符串是否表示失败。
fn is_failure_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "failed" | "error"
    )
}

/// 判断工具是否属于等待用户交互回复的提问工具（不应被归类为后台任务）。
fn is_interactive_question_tool(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "ask_question" | "askquestion" | "elicitation"
    )
}

/// 判断工具在启动参数或名称上是否显式声明为后台/异步任务。
fn is_explicit_background(name: Option<&str>, raw_input: Option<&Value>) -> bool {
    if let Some(inp) = raw_input {
        for key in ["IsDaemon", "is_daemon", "background", "is_background", "async"] {
            if inp.get(key).and_then(Value::as_bool) == Some(true) {
                return true;
            }
        }
    }
    if let Some(n) = name {
        let lower = n.to_ascii_lowercase();
        if matches!(lower.as_str(), "schedule") || lower.contains("background") {
            return true;
        }
    }
    false
}

/// 判断某工具在 Agent 继续生成回复时未结束，是否应自动识别为已转入后台。
fn is_backgroundable_tool(kind: &str, name: &str) -> bool {
    if kind == "execute" {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.contains("command")
        || lower.contains("shell")
        || lower.contains("bash")
        || lower.contains("exec")
        || lower.contains("terminal")
        || lower.contains("schedule")
        || lower.contains("task")
}

/// 提取单行紧凑任务标题（优先取 `toolSummary` / 标题 / 命令行，并折叠换行与多余空白）。
fn build_compact_label(
    title: Option<&str>,
    name: Option<&str>,
    raw_input: Option<&Value>,
) -> Option<String> {
    let raw = raw_input
        .and_then(|v| {
            v.get("toolSummary")
                .and_then(Value::as_str)
                .or_else(|| v.get("Role").and_then(Value::as_str))
                .or_else(|| v.get("Description").and_then(Value::as_str))
        })
        .filter(|s| !s.trim().is_empty())
        .or_else(|| title.filter(|s| !s.trim().is_empty()))
        .or_else(|| {
            raw_input
                .and_then(|v| {
                    v.get("CommandLine")
                        .and_then(Value::as_str)
                        .or_else(|| v.get("command").and_then(Value::as_str))
                })
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| name.filter(|s| !s.trim().is_empty()))?;

    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        None
    } else {
        Some(compact)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn synchronous_tool_completing_in_turn_is_never_marked_background() {
        let mut tracker = BackgroundTracker::default();
        tracker.on_tool_event(
            "c1",
            10,
            Some("run_command"),
            Some("echo hi"),
            Some("execute"),
            Some("in_progress"),
            Some(&json!({ "CommandLine": "echo hi", "IsDaemon": false })),
        );
        assert!(!tracker.has_running());

        let finished = tracker.on_tool_event(
            "c1",
            10,
            None,
            None,
            None,
            Some("completed"),
            None,
        );
        assert!(finished.is_none());
        assert_eq!(tracker.running_count(), 0);
    }

    #[test]
    fn daemon_and_cross_turn_tools_are_tracked_until_terminal_update() {
        let mut tracker = BackgroundTracker::default();
        // 1. 显式 IsDaemon 命令在启动瞬间即标记为后台任务
        tracker.on_tool_event(
            "c_daemon",
            11,
            Some("run_command"),
            Some("npm run dev"),
            Some("execute"),
            Some("in_progress"),
            Some(&json!({
                "CommandLine": "npm run dev",
                "IsDaemon": true,
                "toolSummary": "Dev server startup"
            })),
        );
        assert_eq!(tracker.running_count(), 1);
        assert_eq!(tracker.running_tasks()[0].label, "Dev server startup");

        // 2. 普通耗时命令在回合结束后未完结，自动升级为后台任务
        tracker.on_tool_event(
            "c_bg",
            12,
            Some("run_command"),
            Some("sleep 10 && echo ok"),
            Some("execute"),
            Some("in_progress"),
            Some(&json!({ "CommandLine": "sleep 10 && echo ok", "IsDaemon": false })),
        );
        assert_eq!(tracker.running_count(), 1);
        tracker.on_prompt_finished("end_turn");
        assert_eq!(tracker.running_count(), 2);

        // 3. 后台任务完成后移除并返回完成通知
        let done = tracker.on_tool_event("c_bg", 12, None, None, None, Some("completed"), None);
        assert_eq!(
            done,
            Some(FinishedBackgroundTask {
                label: "sleep 10 && echo ok".into(),
                failed: false,
            })
        );
        assert_eq!(tracker.running_count(), 1);

        let failed = tracker.on_tool_event("c_daemon", 11, None, None, None, Some("failed"), None);
        assert_eq!(
            failed,
            Some(FinishedBackgroundTask {
                label: "Dev server startup".into(),
                failed: true,
            })
        );
        assert!(!tracker.has_running());
    }

    #[test]
    fn agent_replying_while_execute_in_progress_promotes_to_background() {
        let mut tracker = BackgroundTracker::default();
        tracker.on_tool_event(
            "c_exec",
            20,
            Some("run_command"),
            Some("cargo build --release"),
            Some("execute"),
            Some("in_progress"),
            None,
        );
        assert!(!tracker.has_running());

        tracker.on_agent_message_chunk("命令已转入后台运行。");
        assert_eq!(tracker.running_count(), 1);
        assert_eq!(tracker.running_tasks()[0].entry_id, 20);
    }

    #[test]
    fn cancelled_turn_drops_non_background_tools_but_keeps_running_daemons() {
        let mut tracker = BackgroundTracker::default();
        tracker.on_tool_event(
            "c_daemon",
            30,
            Some("run_command"),
            Some("vite"),
            Some("execute"),
            Some("in_progress"),
            Some(&json!({ "IsDaemon": true })),
        );
        tracker.on_tool_event(
            "c_read",
            31,
            Some("read_file"),
            Some("src/main.rs"),
            Some("read"),
            Some("in_progress"),
            None,
        );

        tracker.on_prompt_finished("cancelled");
        assert_eq!(tracker.running_count(), 1);
        assert_eq!(tracker.running_tasks()[0].call_id, "c_daemon");
    }

    #[test]
    fn subagent_family_tools_are_excluded_from_background_tracker() {
        let mut tracker = BackgroundTracker::default();
        let started = tracker.on_tool_event(
            "c_sub",
            40,
            Some("invoke_subagent"),
            Some("Codebase Researcher"),
            Some("other"),
            Some("in_progress"),
            Some(&json!({ "Role": "Codebase Researcher" })),
        );
        assert!(started.is_none());
        assert!(!tracker.has_running());

        let finished = tracker.on_tool_event(
            "c_sub",
            40,
            None,
            None,
            None,
            Some("completed"),
            None,
        );
        assert!(finished.is_none());
        assert_eq!(tracker.running_count(), 0);
    }
}
