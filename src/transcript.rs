//! 会话对话记录与格式化工具。

use std::time::{Duration, Instant};

use chrono::Local;
use serde_json::Value;

use crate::acp::{PlanItem, ToolDiff, TurnUsage};

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
    Plan(Vec<PlanItem>),
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
    pub context_used: u64,
    pub context_window: u64,
    pub total_input: u64,
    pub total_output: u64,
    pub total_cached_read: u64,
    pub turn_started: Option<Instant>,
    next_id: u64,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            session_id: None,
            title: None,
            entries: Vec::new(),
            context_used: 0,
            context_window: 0,
            total_input: 0,
            total_output: 0,
            total_cached_read: 0,
            turn_started: None,
            next_id: 1,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.title = None;
        self.context_used = 0;
        self.total_input = 0;
        self.total_output = 0;
        self.total_cached_read = 0;
        self.turn_started = None;
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn push_user(&mut self, text: String, images: Vec<ImageChip>) {
        if let Some(last) = self.entries.last_mut() {
            if matches!(last.kind, EntryKind::User { .. })
                && self.turn_started.is_none()
                && last.text.is_empty()
            {
                last.text = text;
                last.kind = EntryKind::User { images };
                return;
            }
        }
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text,
            kind: EntryKind::User { images },
        });
    }

    pub fn append_user_chunk(&mut self, chunk: &str) {
        if let Some(last) = self.entries.last_mut() {
            if matches!(last.kind, EntryKind::User { .. }) {
                last.text.push_str(chunk);
                return;
            }
        }
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text: chunk.to_string(),
            kind: EntryKind::User { images: Vec::new() },
        });
    }

    pub fn append_thought(&mut self, chunk: &str) {
        if let Some(last) = self.entries.last_mut() {
            if let EntryKind::Thought { elapsed, .. } = &mut last.kind {
                if elapsed.is_none() {
                    last.text.push_str(chunk);
                    return;
                }
            }
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
            {
                if elapsed.is_none() {
                    *elapsed = Some(started.elapsed());
                }
            }
        }
    }

    pub fn append_assistant(&mut self, chunk: &str) {
        self.finish_open_thoughts();
        if let Some(last) = self.entries.last_mut() {
            if let EntryKind::Assistant { done_line } = &last.kind {
                if done_line.is_none() {
                    last.text.push_str(chunk);
                    return;
                }
            }
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
            {
                if *cid == call_id {
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
        }

        let id = self.alloc_id();
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

    pub fn update_plan(&mut self, items: Vec<PlanItem>) {
        if let Some(last) = self.entries.last_mut() {
            if let EntryKind::Plan(existing) = &mut last.kind {
                *existing = items;
                return;
            }
        }
        let id = self.alloc_id();
        self.entries.push(Entry {
            id,
            text: String::new(),
            kind: EntryKind::Plan(items),
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
        let elapsed = self
            .turn_started
            .take()
            .map(|t| t.elapsed())
            .unwrap_or_default();

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
        if let Some(last) = self.entries.last_mut() {
            if let EntryKind::Assistant { done_line } = &mut last.kind
                && done_line.is_none()
            {
                *done_line = Some(meta);
                return;
            }
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

    pub fn toggle_entry(&mut self, id: u64) -> bool {
        for e in &mut self.entries {
            if e.id == id {
                match &mut e.kind {
                    EntryKind::Thought { expanded, .. } => {
                        *expanded = !*expanded;
                        return true;
                    }
                    EntryKind::Tool { expanded, .. } => {
                        *expanded = !*expanded;
                        return true;
                    }
                    _ => {}
                }
            }
        }
        false
    }

    pub fn toggle_all_details(&mut self, expand: bool) {
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
