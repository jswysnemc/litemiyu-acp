//! 会话对话记录与格式化工具。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::Local;
use serde_json::Value;

use crate::acp::{PlanItem, ToolDiff, TurnUsage};

mod fold;
pub use fold::{FoldReveal, FoldSection};

/// 工具条目的合并状态快照（供子代理追踪器读取完整入参、输出与状态）。
#[derive(Debug, Clone)]
pub struct ToolSnapshot<'a> {
    pub entry_id: u64,
    pub name: &'a str,
    pub title: &'a str,
    pub status: &'a str,
    pub raw_input: Option<&'a Value>,
    pub output: &'a str,
}

#[derive(Debug, Clone)]
pub struct ImageChip {
    pub label: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EntryKind {
    User {
        images: Vec<ImageChip>,
    },
    Thought {
        started: Instant,
        elapsed: Option<Duration>,
        expanded: bool,
    },
    Tool {
        call_id: String,
        name: String,
        title: String,
        kind: String,
        status: String,
        raw_input: Option<Value>,
        output: String,
        diffs: Vec<ToolDiff>,
        expanded: bool,
    },
    Assistant {
        done_line: Option<String>,
    },
    Plan {
        items: Vec<PlanItem>,
        expanded: bool,
    },
    Notice {
        is_error: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u64,
    pub text: String,
    pub kind: EntryKind,
}

pub struct Transcript {
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub entries: Vec<Entry>,
    pub latest_plan: Vec<PlanItem>,
    pub latest_plan_entry_id: Option<u64>,
    pub context_used: u64,
    pub context_window: u64,
    pub total_input: u64,
    pub total_output: u64,
    pub total_cached_read: u64,
    pub turn_started: Option<Instant>,
    pub turn_first_token: Option<Instant>,
    turn_stream_chunks: usize,
    fold_reveals: HashMap<(u64, FoldSection), FoldReveal>,
    full_file_reads: HashSet<u64>,
    next_id: u64,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            session_id: None,
            title: None,
            entries: Vec::new(),
            latest_plan: Vec::new(),
            latest_plan_entry_id: None,
            context_used: 0,
            context_window: 0,
            total_input: 0,
            total_output: 0,
            total_cached_read: 0,
            turn_started: None,
            turn_first_token: None,
            turn_stream_chunks: 0,
            fold_reveals: HashMap::new(),
            full_file_reads: HashSet::new(),
            next_id: 1,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.latest_plan.clear();
        self.latest_plan_entry_id = None;
        self.title = None;
        self.context_used = 0;
        self.total_input = 0;
        self.total_output = 0;
        self.total_cached_read = 0;
        self.turn_started = None;
        self.turn_first_token = None;
        self.turn_stream_chunks = 0;
        self.fold_reveals.clear();
        self.full_file_reads.clear();
    }

    /// 标记新回合开始，重置首字延迟与流式输出块计数。
    pub fn begin_turn(&mut self) {
        self.turn_started = Some(Instant::now());
        self.turn_first_token = None;
        self.turn_stream_chunks = 0;
    }

    /// 记录当前活跃回合首个模型输出到达时刻并累计流式输出块数。
    fn note_turn_token(&mut self) {
        if self.turn_started.is_some() {
            self.turn_first_token.get_or_insert_with(Instant::now);
            self.turn_stream_chunks = self.turn_stream_chunks.saturating_add(1);
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn push_user(&mut self, text: String, images: Vec<ImageChip>) {
        if let Some(last) = self.entries.last_mut()
            && matches!(last.kind, EntryKind::User { .. })
                && self.turn_started.is_none()
                && last.text.is_empty()
            {
                last.text = text;
                last.kind = EntryKind::User { images };
                return;
            }
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text,
            kind: EntryKind::User { images },
        });
    }

    pub fn append_user_chunk(&mut self, chunk: &str) {
        if let Some(last) = self.entries.last_mut()
            && matches!(last.kind, EntryKind::User { .. }) {
                last.text.push_str(chunk);
                return;
            }
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text: chunk.to_string(),
            kind: EntryKind::User { images: Vec::new() },
        });
    }

    pub fn append_thought(&mut self, chunk: &str) {
        if !chunk.is_empty() {
            self.note_turn_token();
        }
        if let Some(last) = self.entries.last_mut()
            && let EntryKind::Thought { elapsed, .. } = &mut last.kind
                && elapsed.is_none() {
                    last.text.push_str(chunk);
                    return;
                }
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text: chunk.to_string(),
            kind: EntryKind::Thought {
                started: Instant::now(),
                elapsed: None,
                expanded: false,
            },
        });
    }

    pub fn finish_open_thoughts(&mut self) {
        for e in self.entries.iter_mut().rev() {
            if let EntryKind::Thought {
                started, elapsed, ..
            } = &mut e.kind
                && elapsed.is_none() {
                    *elapsed = Some(started.elapsed());
                }
        }
    }

    pub fn append_assistant(&mut self, chunk: &str) {
        if !chunk.is_empty() {
            self.note_turn_token();
        }
        self.finish_open_thoughts();
        if let Some(last) = self.entries.last_mut()
            && let EntryKind::Assistant { done_line } = &last.kind
                && done_line.is_none() {
                    last.text.push_str(chunk);
                    return;
                }
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text: chunk.to_string(),
            kind: EntryKind::Assistant { done_line: None },
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_tool(
        &mut self,
        call_id: String,
        name: Option<String>,
        title: Option<String>,
        kind: Option<String>,
        status: Option<String>,
        raw_input: Option<Value>,
        output: Option<String>,
        diffs: Vec<ToolDiff>,
    ) -> u64 {
        self.finish_open_thoughts();
        for e in self.entries.iter_mut().rev() {
            if let EntryKind::Tool {
                call_id: cid,
                name: n,
                title: t,
                kind: k,
                status: s,
                raw_input: ri,
                output: out,
                diffs: existing_diffs,
                ..
            } = &mut e.kind
                && *cid == call_id {
                    if let Some(v) = name {
                        *n = v;
                    }
                    if let Some(v) = title {
                        *t = v;
                    }
                    if let Some(v) = kind {
                        *k = v;
                    }
                    if let Some(v) = status {
                        *s = v;
                    }
                    if let Some(v) = raw_input {
                        *ri = Some(v);
                    }
                    if let Some(v) = output {
                        *out = v;
                    }
                    if !diffs.is_empty() {
                        *existing_diffs = diffs;
                    }
                    return e.id;
                }
        }

        let id = self.alloc_id();
        self.note_turn_token();
        let name_val = name.unwrap_or_else(|| "tool".into());
        let title_val = title.unwrap_or_else(|| name_val.clone());
        self.entries.push(Entry {
            id,
            text: String::new(),
            kind: EntryKind::Tool {
                call_id,
                name: name_val,
                title: title_val,
                kind: kind.unwrap_or_else(|| "other".into()),
                status: status.unwrap_or_else(|| "in_progress".into()),
                raw_input,
                output: output.unwrap_or_default(),
                diffs,
                expanded: false,
            },
        });
        id
    }

    /// 更新当前会话的任务清单：同步持久化最新状态，并在当前回合内原地更新已有任务卡片避免刷屏。
    ///
    /// 参数:
    /// - `items`: 最新的完整任务步骤列表
    pub fn update_plan(&mut self, items: Vec<PlanItem>) {
        self.latest_plan = items.clone();
        if items.is_empty() {
            self.latest_plan_entry_id = None;
            // 1. 收到空任务清单时同步移除当前回合已有的任务卡片，避免正文与底栏状态不一致
            let mut remove_idx = None;
            for (idx, e) in self.entries.iter().enumerate().rev() {
                if matches!(e.kind, EntryKind::User { .. }) {
                    break;
                }
                if matches!(e.kind, EntryKind::Plan { .. }) {
                    remove_idx = Some(idx);
                    break;
                }
            }
            if let Some(idx) = remove_idx {
                self.entries.remove(idx);
            }
            return;
        }
        // 2. 若当前回合（最近一条用户消息之后）已存在任务卡片，则原地更新内容，不重复堆叠多张清单
        for e in self.entries.iter_mut().rev() {
            if matches!(e.kind, EntryKind::User { .. }) {
                break;
            }
            if let EntryKind::Plan { items: existing, .. } = &mut e.kind {
                *existing = items;
                self.latest_plan_entry_id = Some(e.id);
                return;
            }
        }
        // 3. 当前回合首次出现任务清单时新建单行折叠卡片（完整清单固定展示在底部任务栏）
        let id = self.alloc_id();
        self.latest_plan_entry_id = Some(id);
        self.entries.push(Entry {
            id,
            text: String::new(),
            kind: EntryKind::Plan {
                items,
                expanded: false,
            },
        });
    }

    pub fn push_notice(&mut self, text: impl Into<String>, is_error: bool) {
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text: text.into(),
            kind: EntryKind::Notice { is_error },
        });
    }

    pub fn finish_turn(&mut self, model_label: &str, stop_reason: &str, usage: Option<&TurnUsage>) {
        self.finish_open_thoughts();
        let finished_at = Instant::now();
        let started = self.turn_started.take();
        let first_token = self.turn_first_token.take();
        let stream_chunks = std::mem::take(&mut self.turn_stream_chunks);
        let elapsed = started
            .map(|t| finished_at.saturating_duration_since(t))
            .unwrap_or_default();
        let ttft = started
            .zip(first_token)
            .map(|(s, f)| f.saturating_duration_since(s));

        if let Some(u) = usage {
            self.total_input = self.total_input.saturating_add(u.input_tokens);
            self.total_output = self.total_output.saturating_add(u.output_tokens);
            self.total_cached_read = self.total_cached_read.saturating_add(u.cached_read_tokens);
        }

        let now = Local::now().format("%H:%M").to_string();
        let mut parts = vec![now];
        if !model_label.is_empty() {
            parts.push(model_label.to_string());
        }
        if elapsed.as_millis() > 0 {
            parts.push(format_duration(elapsed));
        }
        // 1. 首字延迟（TTFT）：过滤掉异步续答瞬时启动产生的 <1ms 伪零值
        if let Some(latency) = ttft
            && latency >= Duration::from_millis(1)
        {
            parts.push(format!("首字 {}", format_latency(latency)));
        }
        // 2. Token 生成速度：多流式块扣除首字等待时间，单块回退总耗时，避免分母过小失真
        if let Some(u) = usage {
            let gen_tokens = generated_tokens(u);
            if gen_tokens > 0 {
                let decode_dur = if stream_chunks >= 2 {
                    let after_first = ttft
                        .map(|t| elapsed.saturating_sub(t))
                        .unwrap_or(elapsed);
                    if after_first >= Duration::from_millis(50) {
                        after_first
                    } else {
                        elapsed
                    }
                } else {
                    elapsed
                };
                if decode_dur >= Duration::from_millis(50) {
                    let tps = gen_tokens as f64 / decode_dur.as_secs_f64();
                    parts.push(format_token_speed(tps));
                }
            }
        }
        if self.context_used > 0 {
            let mut ctx = format_short(self.context_used);
            if let Some(u) = usage {
                let denom = u.input_tokens + u.cached_read_tokens;
                if denom > 0 && u.cached_read_tokens > 0 {
                    let pct = (u.cached_read_tokens * 100) / denom;
                    ctx.push_str(&format!("(C{pct}%)"));
                }
            }
            parts.push(ctx);
        }
        if stop_reason == "cancelled" {
            parts.push("已中断".into());
        }
        let meta = format!("▣ {}", parts.join(" · "));

        // 1. 末条为未关闭的助手回答时直接附着轮次结束行
        if let Some(last) = self.entries.last_mut()
            && let EntryKind::Assistant { done_line } = &mut last.kind
                && done_line.is_none()
            {
                *done_line = Some(meta);
                return;
            }
        // 2. 本轮以工具/计划/思考结尾或被中断时，追加独立的轮次结束行，避免 ▣ 汇总丢失
        let has_turn_output = self
            .entries
            .iter()
            .rev()
            .take_while(|e| !matches!(e.kind, EntryKind::User { .. }))
            .any(|e| {
                !matches!(
                    &e.kind,
                    EntryKind::Assistant { done_line: Some(_) } if e.text.trim().is_empty()
                )
            });
        if has_turn_output || stop_reason == "cancelled" {
            let id = self.alloc_id();
            self.entries.push(Entry {
                id,
                text: String::new(),
                kind: EntryKind::Assistant {
                    done_line: Some(meta),
                },
            });
        }
    }

    /// 显式设置思考或工具条目的展开状态（侧栏点击子代理自动展开时使用）。
    ///
    /// 参数:
    /// - `id`: 时间线条目 ID
    /// - `expand`: 目标展开状态
    ///
    /// 返回:
    /// - 找到并更新目标条目时返回 `true`
    pub fn set_entry_expanded(&mut self, id: u64, expand: bool) -> bool {
        let mut matched = false;
        for e in &mut self.entries {
            if e.id == id {
                match &mut e.kind {
                    EntryKind::Thought { expanded, .. }
                    | EntryKind::Tool { expanded, .. }
                    | EntryKind::Plan { expanded, .. } => {
                        *expanded = expand;
                        matched = true;
                    }
                    _ => {}
                }
                break;
            }
        }
        if matched && !expand {
            self.reset_entry_reveals(id);
        }
        matched
    }

    /// 根据 ACP `call_id` 读取已合并的工具条目快照。
    ///
    /// 参数:
    /// - `call_id`: ACP 工具调用唯一标识
    ///
    /// 返回:
    /// - 找到匹配工具条目时返回 `Some(ToolSnapshot)`
    pub fn tool_snapshot(&self, call_id: &str) -> Option<ToolSnapshot<'_>> {
        for e in self.entries.iter().rev() {
            if let EntryKind::Tool {
                call_id: cid,
                name,
                title,
                status,
                raw_input,
                output,
                ..
            } = &e.kind
                && cid == call_id
            {
                return Some(ToolSnapshot {
                    entry_id: e.id,
                    name,
                    title,
                    status,
                    raw_input: raw_input.as_ref(),
                    output,
                });
            }
        }
        None
    }

    pub fn toggle_entry(&mut self, id: u64) -> bool {
        let partial_thought = self.fold_reveal(id, FoldSection::Thought).effective_limit(3) > 3;
        let mut collapsed = false;
        let mut matched = false;
        for e in &mut self.entries {
            if e.id == id {
                match &mut e.kind {
                    EntryKind::Thought { expanded, .. } => {
                        *expanded = !*expanded && !partial_thought;
                        collapsed = !*expanded;
                        matched = true;
                        break;
                    }
                    EntryKind::Tool { expanded, .. } | EntryKind::Plan { expanded, .. } => {
                        *expanded = !*expanded;
                        collapsed = !*expanded;
                        matched = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
        if collapsed {
            self.reset_entry_reveals(id);
        }
        matched
    }

    pub fn toggle_all_details(&mut self, expand: bool) {
        self.fold_reveals.clear();
        self.full_file_reads.clear();
        for e in &mut self.entries {
            match &mut e.kind {
                EntryKind::Thought { expanded, .. } => *expanded = expand,
                EntryKind::Tool { expanded, .. } => *expanded = expand,
                _ => {}
            }
        }
    }

    pub fn nth_assistant_text(&self, n: usize) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .filter(|e| matches!(e.kind, EntryKind::Assistant { .. }) && !e.text.trim().is_empty())
            .nth(n.saturating_sub(1))
            .map(|e| e.text.as_str())
    }

    pub fn full_conversation_markdown(&self) -> String {
        let mut out = Vec::new();
        for e in &self.entries {
            match &e.kind {
                EntryKind::User { .. } => out.push(format!("> **User**\n>\n> {}", e.text.replace('\n', "\n> "))),
                EntryKind::Assistant { .. } if !e.text.trim().is_empty() => out.push(e.text.clone()),
                _ => {}
            }
        }
        out.join("\n\n---\n\n")
    }

    pub fn user_prompts(&self, limit: usize) -> Vec<(u64, String)> {
        let mut list: Vec<(u64, String)> = self
            .entries
            .iter()
            .rev()
            .filter(|e| matches!(e.kind, EntryKind::User { .. }))
            .take(limit)
            .map(|e| {
                let first = e
                    .text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                (e.id, first)
            })
            .collect();
        list.reverse();
        list
    }
}

pub fn format_short(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round() as u64)
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 60.0 {
        let m = (secs / 60.0).floor() as u64;
        let s = (secs % 60.0).round() as u64;
        format!("{m}m{s}s")
    } else {
        format!("{secs:.1}s")
    }
}

pub fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 8 {
        id.to_string()
    } else {
        chars[..8].iter().collect()
    }
}

/// 计算本轮模型实际生成的输出 Token 数（兼容 thoughtTokens 独立上报或已折入 outputTokens 的后端）。
///
/// 参数:
/// - `u`: 本轮 ACP 结束响应返回的 `TurnUsage`
///
/// 返回:
/// - 本轮生成的总输出 Token 数量
fn generated_tokens(u: &TurnUsage) -> u64 {
    if u.thought_tokens == 0 {
        return u.output_tokens;
    }
    let sum_without_thought = u.input_tokens.saturating_add(u.output_tokens);
    let sum_with_cached = sum_without_thought.saturating_add(u.cached_read_tokens);
    if u.total_tokens > 0
        && (u.total_tokens == sum_without_thought || u.total_tokens == sum_with_cached)
    {
        u.output_tokens
    } else {
        u.output_tokens.saturating_add(u.thought_tokens)
    }
}

/// 格式化首字延迟（小于 1 秒用 `ms`，大于等于 1 秒复用 `format_duration`）。
fn format_latency(d: Duration) -> String {
    if d < Duration::from_secs(1) {
        format!("{}ms", d.as_millis())
    } else {
        format_duration(d)
    }
}

/// 格式化 Token 生成速度（`>= 100` 取整，其余保留 1 位小数）。
fn format_token_speed(tps: f64) -> String {
    if tps >= 100.0 {
        format!("{tps:.0} tok/s")
    } else {
        format!("{tps:.1} tok/s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_turn_includes_ttft_and_token_speed() {
        let mut tr = Transcript::new();
        tr.context_used = 156_700;
        let now = Instant::now();
        tr.turn_started = Some(now - Duration::from_secs(10));
        tr.turn_first_token = Some(now - Duration::from_secs(8));
        tr.turn_stream_chunks = 5;
        tr.append_assistant("回答正文");

        let usage = TurnUsage {
            input_tokens: 2_000,
            output_tokens: 400,
            cached_read_tokens: 98_000,
            thought_tokens: 240,
            total_tokens: 100_640,
        };
        tr.finish_turn("Gemini 4 Neon (High)", "end_turn", Some(&usage));

        let done = match &tr.entries.last().unwrap().kind {
            EntryKind::Assistant { done_line } => done_line.as_deref().unwrap(),
            other => panic!("expected assistant done line, got {other:?}"),
        };
        assert!(done.contains("首字 2.0s"), "missing ttft in done line: {done}");
        // 生成总计 400 + 240 = 640 tokens，解码耗时约 8s -> 约 80.0 tok/s
        assert!(done.contains("tok/s"), "missing token speed in done line: {done}");
        assert!(done.contains("80.0 tok/s"), "unexpected speed in done line: {done}");
        assert!(done.contains("156.7k(C98%)"), "missing context cache in done line: {done}");
    }

    #[test]
    fn generated_tokens_avoids_double_counting_folded_thought_tokens() {
        let disjoint = TurnUsage {
            input_tokens: 1000,
            output_tokens: 300,
            cached_read_tokens: 0,
            thought_tokens: 200,
            total_tokens: 1500,
        };
        assert_eq!(generated_tokens(&disjoint), 500);

        let folded = TurnUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cached_read_tokens: 0,
            thought_tokens: 200,
            total_tokens: 1500,
        };
        assert_eq!(generated_tokens(&folded), 500);

        assert_eq!(format_latency(Duration::from_millis(420)), "420ms");
        assert_eq!(format_latency(Duration::from_millis(1850)), "1.9s");
        assert_eq!(format_token_speed(128.4), "128 tok/s");
        assert_eq!(format_token_speed(42.56), "42.6 tok/s");
    }
}
