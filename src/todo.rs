//! Todo / 任务计划统一解析与状态统计模块（兼容 `agy acp` 的 `update_steps`、标准 ACP `plan` 及常见 Todo 工具）。

use ratatui::style::{Modifier, Style};
use serde_json::Value;

use crate::acp::PlanItem;
use crate::theme;

/// 进行中步骤使用的 4 帧等宽几何旋转指示符（避免 Emoji）。
const IN_PROGRESS_SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// 规范化后的单条任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Skipped,
    Failed,
}

/// 任务列表聚合进度统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TodoCounts {
    pub total: usize,
    pub completed: usize,
    pub in_progress: usize,
    pub pending: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl TodoCounts {
    /// 返回已收尾条目数（包含已完成与已跳过）。
    ///
    /// 返回:
    /// - 已完成的条目数量
    pub fn finished(&self) -> usize {
        self.completed.saturating_add(self.skipped)
    }

    /// 判断所有步骤是否已全部完成（且至少存在 1 条步骤，无失败项）。
    ///
    /// 返回:
    /// - 全部完成或跳过且无失败时返回 `true`
    pub fn all_done(&self) -> bool {
        self.total > 0 && self.failed == 0 && self.finished() == self.total
    }

    /// 计算用于进度条展示的完成比例（`0.0..=1.0`）。
    ///
    /// 返回:
    /// - 完成比例浮点数
    pub fn progress_ratio(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.finished() as f64 / self.total as f64).clamp(0.0, 1.0)
    }
}

/// 判断 ACP 工具名称是否属于任务/待办清单更新工具。
///
/// 参数:
/// - `name`: ACP `tool_call` 上报的工具名
///
/// 返回:
/// - 若该工具用于更新任务清单则返回 `true`
pub fn is_todo_tool(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "update_steps" | "todowrite" | "todo_write" | "write_todos" | "update_plan"
    )
}

/// 将不同后端上报的状态字符串归一化为标准状态标识。
///
/// 参数:
/// - `raw`: 原始状态字符串（如 `done`、`completed`、`in_progress`、`skipped`、`failed`）
///
/// 返回:
/// - 归一化后的静态状态字符串
pub fn normalize_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "completed" | "complete" | "done" | "finished" | "succeeded" => "completed",
        "in_progress" | "in-progress" | "inprogress" | "running" | "active" | "working" => {
            "in_progress"
        }
        "failed" | "error" => "failed",
        "skipped" | "cancelled" | "canceled" => "skipped",
        _ => "pending",
    }
}

/// 将状态字符串解析为强类型 `TodoStatus`。
///
/// 参数:
/// - `raw`: 状态字符串
///
/// 返回:
/// - 对应的 `TodoStatus` 枚举值
pub fn parse_status(raw: &str) -> TodoStatus {
    match normalize_status(raw) {
        "completed" => TodoStatus::Completed,
        "in_progress" => TodoStatus::InProgress,
        "failed" => TodoStatus::Failed,
        "skipped" => TodoStatus::Skipped,
        _ => TodoStatus::Pending,
    }
}

/// 从 Todo 类工具的入参中提取任务清单条目。
///
/// 参数:
/// - `name`: 工具名称
/// - `raw_input`: ACP `rawInput` 或 `args` JSON 值
///
/// 返回:
/// - 若识别到有效任务数组（含空数组清空场景）则返回 `Some(Vec<PlanItem>)`，否则返回 `None`
pub fn extract_plan_items_from_tool(name: &str, raw_input: Option<&Value>) -> Option<Vec<PlanItem>> {
    if !is_todo_tool(name) {
        return None;
    }
    let input = raw_input?;
    // 1. 兼容数组位于 `steps` / `todos` / `plan` / `entries` 字段，或值被二次 JSON 编码的场景
    let mut owned_parsed: Option<Value> = None;
    let mut matched_arr: Option<&[Value]> = None;
    for key in ["steps", "todos", "plan", "entries"] {
        let Some(val) = input.get(key) else {
            continue;
        };
        if let Some(arr) = val.as_array() {
            matched_arr = Some(arr.as_slice());
            break;
        }
        if let Some(s) = val.as_str()
            && let Ok(parsed) = serde_json::from_str::<Value>(s)
            && parsed.is_array()
        {
            owned_parsed = Some(parsed);
            break;
        }
    }
    let array = matched_arr
        .or_else(|| owned_parsed.as_ref().and_then(Value::as_array).map(Vec::as_slice))?;

    // 2. 逐项提取标题与归一化状态，过滤空白标题
    let mut items = Vec::with_capacity(array.len());
    for entry in array {
        let content = ["title", "content", "step", "task", "description", "activeForm"]
            .iter()
            .find_map(|k| entry.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(content) = content else {
            continue;
        };
        let status = entry
            .get("status")
            .and_then(Value::as_str)
            .map(normalize_status)
            .unwrap_or("pending");
        items.push(PlanItem {
            content: content.to_string(),
            status: status.to_string(),
        });
    }
    Some(items)
}

/// 统计任务清单各状态数量。
///
/// 参数:
/// - `items`: 当前任务清单切片
///
/// 返回:
/// - 汇总后的 `TodoCounts`
pub fn count_items(items: &[PlanItem]) -> TodoCounts {
    let mut counts = TodoCounts {
        total: items.len(),
        ..TodoCounts::default()
    };
    for item in items {
        match parse_status(&item.status) {
            TodoStatus::Completed => counts.completed += 1,
            TodoStatus::InProgress => counts.in_progress += 1,
            TodoStatus::Pending => counts.pending += 1,
            TodoStatus::Skipped => counts.skipped += 1,
            TodoStatus::Failed => counts.failed += 1,
        }
    }
    counts
}

/// 选取最值得在单行摘要中聚焦展示的步骤（优先进行中，其次待处理，再次失败项，最后末项）。
///
/// 参数:
/// - `items`: 当前任务清单切片
///
/// 返回:
/// - 聚焦步骤引用
pub fn focus_item(items: &[PlanItem]) -> Option<&PlanItem> {
    items
        .iter()
        .find(|i| parse_status(&i.status) == TodoStatus::InProgress)
        .or_else(|| {
            items
                .iter()
                .find(|i| parse_status(&i.status) == TodoStatus::Pending)
        })
        .or_else(|| {
            items
                .iter()
                .find(|i| parse_status(&i.status) == TodoStatus::Failed)
        })
        .or_else(|| items.last())
}

/// 为窄窗口裁剪可视步骤切片，优先保证当前进行中步骤落在可视窗口内。
///
/// 参数:
/// - `items`: 完整任务列表
/// - `limit`: 最多展示条目数
///
/// 返回:
/// - `(可视切片, 顶部省略数, 底部省略数)`
pub fn visible_window(items: &[PlanItem], limit: usize) -> (&[PlanItem], usize, usize) {
    if items.len() <= limit || limit == 0 {
        return (items, 0, 0);
    }
    let anchor = items
        .iter()
        .position(|i| parse_status(&i.status) == TodoStatus::InProgress)
        .or_else(|| {
            items
                .iter()
                .position(|i| parse_status(&i.status) == TodoStatus::Pending)
        })
        .unwrap_or_else(|| items.len().saturating_sub(1));
    let half = limit / 2;
    let start = anchor
        .saturating_sub(half)
        .min(items.len().saturating_sub(limit));
    let end = (start + limit).min(items.len());
    (&items[start..end], start, items.len().saturating_sub(end))
}

/// 返回任务状态对应的纯几何符号与样式（绝不使用 Emoji）。
///
/// 参数:
/// - `status`: 任务状态
/// - `animate`: 是否对进行中状态启用旋转帧
/// - `tick`: 渲染节拍
///
/// 返回:
/// - `(状态符号, 符号样式, 标题文字样式)`
pub fn status_visual(status: TodoStatus, animate: bool, tick: usize) -> (&'static str, Style, Style) {
    match status {
        TodoStatus::Completed => ("✓", theme::good(), theme::dim()),
        TodoStatus::InProgress => {
            let sym = if animate {
                IN_PROGRESS_SPINNER[tick % IN_PROGRESS_SPINNER.len()]
            } else {
                "◐"
            };
            (sym, theme::accent_bold(), theme::text().add_modifier(Modifier::BOLD))
        }
        TodoStatus::Failed => ("✕", theme::error(), theme::error()),
        TodoStatus::Skipped => ("–", theme::faint(), theme::faint()),
        TodoStatus::Pending => ("○", theme::dim(), theme::text()),
    }
}

/// 构造 `▰`/`▱` 迷你进度条字符串片段。
///
/// 参数:
/// - `counts`: 任务统计数据
/// - `width`: 进度条总列宽
///
/// 返回:
/// - `(已填充段, 未填充段)`
pub fn progress_bar(counts: &TodoCounts, width: usize) -> (String, String) {
    if width == 0 {
        return (String::new(), String::new());
    }
    let raw = (counts.progress_ratio() * width as f64).round() as usize;
    let filled = if counts.finished() > 0 {
        raw.max(1)
    } else {
        0
    }
    .min(width);
    let empty = width.saturating_sub(filled);
    ("▰".repeat(filled), "▱".repeat(empty))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_agy_update_steps_and_normalizes_statuses() {
        assert!(is_todo_tool("update_steps"));
        assert!(is_todo_tool("TodoWrite"));
        assert!(!is_todo_tool("run_command"));

        let raw = json!({
            "steps": [
                { "title": "排查协议", "status": "done" },
                { "title": "编写渲染组件", "status": "in_progress" },
                { "title": "跳过旧方案", "status": "skipped" },
                { "title": "验证失败回退", "status": "failed" },
                { "title": "补充测试", "status": "pending" }
            ]
        });
        let items = extract_plan_items_from_tool("update_steps", Some(&raw)).expect("should parse steps");
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].content, "排查协议");
        assert_eq!(items[0].status, "completed");
        assert_eq!(items[1].status, "in_progress");
        assert_eq!(items[2].status, "skipped");
        assert_eq!(items[3].status, "failed");
        assert_eq!(items[4].status, "pending");

        let counts = count_items(&items);
        assert_eq!(counts.total, 5);
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.finished(), 2);
        assert_eq!(counts.in_progress, 1);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.pending, 1);
        assert!(!counts.all_done());
        assert_eq!(focus_item(&items).map(|i| i.content.as_str()), Some("编写渲染组件"));
    }

    #[test]
    fn visible_window_keeps_in_progress_step_centered() {
        let items: Vec<PlanItem> = (0..10)
            .map(|i| PlanItem {
                content: format!("step {i}"),
                status: if i < 6 {
                    "completed"
                } else if i == 6 {
                    "in_progress"
                } else {
                    "pending"
                }
                .into(),
            })
            .collect();
        let (slice, above, below) = visible_window(&items, 4);
        assert_eq!(slice.len(), 4);
        assert!(slice.iter().any(|i| i.content == "step 6"));
        assert_eq!(above + slice.len() + below, 10);
    }
}
