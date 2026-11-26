//! 正文时间线行构建器与 Markdown 宽度缓存。

use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::acp::ToolDiff;
use crate::diff;
use crate::linebreak;
use crate::markdown::{self, MdLine};
use crate::theme::{self, ThemePreset};
use crate::transcript::{Entry, EntryKind, FoldSection, Transcript, format_duration, format_short};

mod fold;
mod subagent;
mod tool;
mod tool_meta;

use tool::build_tool;

pub(super) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, Clone, Copy, Default)]
pub struct RowCopyMeta {
    /// 正文起始列偏移（相对 `body_area.x`，划词复制时自动跳过左侧装饰列）
    pub content_left: u16,
    /// 纯装饰行（如代码块语言标签行、用户消息上下空边框），多行框选时自动跳过
    pub decorative: bool,
    /// 软折行产生的续行，复制时无缝拼接到上一行
    pub join_previous: bool,
}

#[derive(Debug, Clone)]
pub struct RenderRow {
    pub entry_id: u64,
    pub spans: Vec<Span<'static>>,
    pub links: Vec<(u16, u16, String)>,
    pub action: RowAction,
    pub copy_meta: RowCopyMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    None,
    Toggle,
    Fold(FoldSection),
}

#[derive(Hash, PartialEq, Eq)]
struct CacheKey {
    entry_id: u64,
    width: u16,
    preset: u8,
    paint_bg: bool,
}

#[derive(Hash, PartialEq, Eq)]
struct ThoughtCacheKey {
    entry_id: u64,
    width: u16,
}

pub struct MdCache {
    entries: HashMap<CacheKey, (usize, Vec<MdLine>)>,
    thoughts: HashMap<ThoughtCacheKey, (usize, Vec<String>)>,
    diffs: HashMap<(u64, usize), (usize, usize, diff::ComputedDiff)>,
}

impl MdCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            thoughts: HashMap::new(),
            diffs: HashMap::new(),
        }
    }

    /// 渲染或命中缓存的 Markdown 行集合（同一 entry 流式增长时原地覆盖，避免清空历史缓存）。
    ///
    /// 参数:
    /// - `entry_id`: 对话条目唯一 ID
    /// - `text`: Markdown 源码
    /// - `width`: 渲染宽度
    ///
    /// 返回:
    /// - 已渲染的 `MdLine` 切片
    pub fn render(&mut self, entry_id: u64, text: &str, width: u16) -> &[MdLine] {
        let key = CacheKey {
            entry_id,
            width,
            preset: preset_id(theme::current_preset()),
            paint_bg: theme::paint_bg(),
        };
        if self.entries.len() > 512 && !self.entries.contains_key(&key) {
            self.entries.clear();
        }
        let text_len = text.len();
        let slot = self
            .entries
            .entry(key)
            .or_insert_with(|| (usize::MAX, Vec::new()));
        if slot.0 != text_len {
            slot.0 = text_len;
            slot.1 = markdown::render(text, width);
        }
        &slot.1
    }

    /// 缓存思考文本按宽度的折行结果，避免每 32ms 重跑历史思考的字素切分与折行。
    ///
    /// 参数:
    /// - `entry_id`: 思考条目 ID
    /// - `trimmed`: 去除首尾空白后的思考文本
    /// - `inner_w`: 可用列宽
    ///
    /// 返回:
    /// - 折行后的字符串切片
    pub fn wrap_thought(&mut self, entry_id: u64, trimmed: &str, inner_w: usize) -> &[String] {
        let key = ThoughtCacheKey {
            entry_id,
            width: u16::try_from(inner_w).unwrap_or(u16::MAX),
        };
        if self.thoughts.len() > 512 && !self.thoughts.contains_key(&key) {
            self.thoughts.clear();
        }
        let text_len = trimmed.len();
        let slot = self
            .thoughts
            .entry(key)
            .or_insert_with(|| (usize::MAX, Vec::new()));
        if slot.0 != text_len {
            let mut wrapped = Vec::new();
            for raw_line in trimmed.lines() {
                let cells: Vec<&str> = raw_line.graphemes(true).collect();
                for range in linebreak::lines(&cells, inner_w) {
                    wrapped.push(cells[range].concat());
                }
            }
            slot.0 = text_len;
            slot.1 = wrapped;
        }
        &slot.1
    }

    /// 缓存工具调用的 Myers Diff 计算结果，避免折叠或历史编辑记录每帧重复计算。
    ///
    /// 参数:
    /// - `entry_id`: 工具条目 ID
    /// - `diff_idx`: 条目内第几个文件 diff
    /// - `td`: 原始新旧文本
    ///
    /// 返回:
    /// - 缓存的 `ComputedDiff` 副本
    pub fn compute_diff(
        &mut self,
        entry_id: u64,
        diff_idx: usize,
        td: &ToolDiff,
    ) -> diff::ComputedDiff {
        let key = (entry_id, diff_idx);
        if self.diffs.len() > 512 && !self.diffs.contains_key(&key) {
            self.diffs.clear();
        }
        let old_len = td.old_text.len();
        let new_len = td.new_text.len();
        if let Some((ol, nl, cached)) = self.diffs.get(&key)
            && *ol == old_len
            && *nl == new_len
        {
            return cached.clone();
        }
        let computed = diff::compute(&td.old_text, &td.new_text);
        self.diffs.insert(key, (old_len, new_len, computed.clone()));
        computed
    }
}

fn preset_id(p: ThemePreset) -> u8 {
    match p {
        ThemePreset::MiyuLavender => 0,
        ThemePreset::MiyuNight => 1,
        ThemePreset::MiyuForest => 2,
        ThemePreset::MiyuDay => 3,
        ThemePreset::MiyuPaper => 4,
        ThemePreset::Terminal => 5,
    }
}

pub struct BuiltBody {
    pub rows: Vec<RenderRow>,
    pub entry_starts: HashMap<u64, usize>,
}

pub fn build_body(
    transcript: &Transcript,
    subagents: &[crate::subagent::Agent],
    width: u16,
    tick: usize,
    cache: &mut MdCache,
) -> BuiltBody {
    let width = width.max(16);
    let mut rows = Vec::new();
    let mut entry_starts = HashMap::new();

    for (idx, entry) in transcript.entries.iter().enumerate() {
        if idx > 0 {
            let need_gap = !matches!(
                (&transcript.entries[idx - 1].kind, &entry.kind),
                (EntryKind::Tool { .. }, EntryKind::Tool { .. })
            );
            if need_gap {
                rows.push(blank_row(entry.id));
            }
        }
        entry_starts.insert(entry.id, rows.len());
        match &entry.kind {
            EntryKind::User { images } => build_user(entry, images, width, &mut rows),
            EntryKind::Thought {
                started,
                elapsed,
                expanded,
            } => build_thought(
                entry,
                transcript.fold_reveal(entry.id, FoldSection::Thought).effective_limit(3),
                *started,
                *elapsed,
                *expanded,
                width,
                tick,
                cache,
                &mut rows,
            ),
            EntryKind::Tool { expanded, name, .. } if crate::subagent::is_subagent_family_tool(name)
                && subagents.iter().any(|agent| agent.entry_id == entry.id) => {
                subagent::append(entry.id, subagents, *expanded, width, &mut rows);
            }
            EntryKind::Tool {
                name,
                title,
                kind,
                status,
                raw_input,
                output,
                diffs,
                expanded,
                ..
            } => build_tool(
                entry.id,
                transcript,
                name,
                title,
                kind,
                status,
                raw_input.as_ref(),
                output,
                diffs,
                *expanded,
                width,
                tick,
                cache,
                &mut rows,
            ),
            EntryKind::Assistant { done_line } => {
                build_assistant(entry, done_line.as_deref(), width, cache, &mut rows)
            }
            EntryKind::Plan(items) => build_plan(entry.id, items, width, &mut rows),
            EntryKind::Notice { is_error } => build_notice(entry, *is_error, width, &mut rows),
        }
    }

    BuiltBody { rows, entry_starts }
}

fn build_user(
    entry: &Entry,
    images: &[crate::transcript::ImageChip],
    width: u16,
    out: &mut Vec<RenderRow>,
) {
    let bar = Span::styled("┃ ", theme::user_bar());
    let inner_w = usize::from(width.saturating_sub(2)).max(1);
    let chip_style = theme::md_image().bg(theme::palette().surface);

    out.push(RenderRow {
        entry_id: entry.id,
        spans: vec![bar.clone()],
        links: Vec::new(),
        action: RowAction::None,
        copy_meta: RowCopyMeta {
            content_left: 2,
            decorative: true,
            join_previous: false,
        },
    });
    for raw_line in entry.text.trim_matches('\n').split('\n') {
        let cells: Vec<&str> = raw_line.graphemes(true).collect();
        for (wrap_idx, range) in linebreak::lines(&cells, inner_w).into_iter().enumerate() {
            let text = cells[range].concat();
            let (styled_spans, mut line_links) = style_image_chips(&text, images, chip_style);
            // 加上左侧 `┃ ` 的 2 列偏移，让鼠标点击精准落在 `[图片 N]` 上
            for (a, b, _) in &mut line_links {
                *a = a.saturating_add(2);
                *b = b.saturating_add(2);
            }
            let mut spans = Vec::with_capacity(1 + styled_spans.len());
            spans.push(bar.clone());
            spans.extend(styled_spans);
            out.push(RenderRow {
                entry_id: entry.id,
                spans,
                links: line_links,
                action: RowAction::None,
                copy_meta: RowCopyMeta {
                    content_left: 2,
                    decorative: false,
                    join_previous: wrap_idx > 0,
                },
            });
        }
    }
    out.push(RenderRow {
        entry_id: entry.id,
        spans: vec![bar],
        links: Vec::new(),
        action: RowAction::None,
        copy_meta: RowCopyMeta {
            content_left: 2,
            decorative: true,
            join_previous: false,
        },
    });
}

fn style_image_chips(
    line: &str,
    images: &[crate::transcript::ImageChip],
    chip_style: Style,
) -> (Vec<Span<'static>>, Vec<(u16, u16, String)>) {
    let mut spans = Vec::new();
    let mut links = Vec::new();
    let mut rest = line;
    let mut col = 0u16;

    while !rest.is_empty() {
        let found = rest.find("[图片").and_then(|start| {
            let tail = &rest[start..];
            let close = tail.find(']')?;
            let end = start + close + 1;
            Some((start, end))
        });
        let Some((start, end)) = found else {
            spans.push(Span::styled(rest.to_string(), theme::text()));
            break;
        };
        if start > 0 {
            let before = &rest[..start];
            col = col.saturating_add(u16::try_from(before.width()).unwrap_or(0));
            spans.push(Span::styled(before.to_string(), theme::text()));
        }
        let token = &rest[start..end];
        let w = u16::try_from(token.width()).unwrap_or(0);
        if let Some(path) = images
            .iter()
            .find(|c| c.label == token)
            .and_then(|c| c.path.clone())
        {
            links.push((col, col.saturating_add(w), path));
        }
        col = col.saturating_add(w);
        spans.push(Span::styled(token.to_string(), chip_style));
        rest = &rest[end..];
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    (spans, links)
}

#[allow(clippy::too_many_arguments)]
fn build_thought(
    entry: &Entry,
    preview_limit: usize,
    started: std::time::Instant,
    elapsed: Option<std::time::Duration>,
    expanded: bool,
    width: u16,
    tick: usize,
    cache: &mut MdCache,
    out: &mut Vec<RenderRow>,
) {
    let busy = elapsed.is_none();
    let dur = elapsed.unwrap_or_else(|| started.elapsed());
    let icon = if busy {
        SPINNER[tick % SPINNER.len()]
    } else {
        "◇"
    };
    let word = if busy { "思考中" } else { "思考" };
    let est_tokens = entry.text.trim().chars().count() as u64 / 3;
    let mut header = format!("{icon} {word} · {}", format_duration(dur));
    if est_tokens > 0 {
        header.push_str(&format!(" · {}", format_short(est_tokens)));
    }
    let hint = if expanded || preview_limit > 3 { " ▾" } else { " ▸" };
    let header_style = if busy { theme::accent() } else { theme::dim() };

    out.push(RenderRow {
        entry_id: entry.id,
        spans: vec![
            Span::styled(linebreak::clip(&header, width.saturating_sub(3)), header_style),
            Span::styled(hint, theme::faint()),
        ],
        links: Vec::new(),
        action: RowAction::Toggle,
        copy_meta: RowCopyMeta::default(),
    });

    let trimmed = entry.text.trim();
    if trimmed.is_empty() {
        return;
    }

    let inner_w = usize::from(width.saturating_sub(2)).max(1);
    let wrapped = cache.wrap_thought(entry.id, trimmed, inner_w);

    let shown: &[String] = if expanded || wrapped.len() <= preview_limit {
        wrapped
    } else {
        &wrapped[wrapped.len() - preview_limit..]
    };

    if !expanded && wrapped.len() > preview_limit {
        out.push(fold::marker(entry.id, FoldSection::Thought, wrapped.len() - preview_limit));
    }

    for line in shown {
        out.push(RenderRow {
            entry_id: entry.id,
            spans: vec![
                Span::styled("│ ", theme::faint()),
                Span::styled(line.clone(), theme::thought()),
            ],
            links: Vec::new(),
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: false,
                join_previous: false,
            },
        });
    }
}

fn build_assistant(
    entry: &Entry,
    done_line: Option<&str>,
    width: u16,
    cache: &mut MdCache,
    out: &mut Vec<RenderRow>,
) {
    let md_lines = cache.render(entry.id, &entry.text, width);
    for line in md_lines {
        let lead_w: u16 = line
            .lead
            .iter()
            .map(|s| u16::try_from(s.content.width()).unwrap_or(0))
            .sum();
        let links = line
            .folded
            .links
            .iter()
            .map(|(a, b, url)| (a.saturating_add(lead_w), b.saturating_add(lead_w), url.clone()))
            .collect();
        out.push(RenderRow {
            entry_id: entry.id,
            spans: line.to_spans(),
            links,
            action: RowAction::None,
            copy_meta: RowCopyMeta {
                content_left: line.content_offset,
                decorative: line.decorative,
                join_previous: line.folded.joined,
            },
        });
    }

    if let Some(meta) = done_line {
        if !entry.text.trim().is_empty() {
            out.push(blank_row(entry.id));
        }
        out.push(RenderRow {
            entry_id: entry.id,
            spans: vec![Span::styled(
                linebreak::clip(meta, width),
                theme::faint(),
            )],
            links: Vec::new(),
            action: RowAction::None,
            copy_meta: RowCopyMeta::default(),
        });
    }
}

fn build_plan(
    entry_id: u64,
    items: &[crate::acp::PlanItem],
    width: u16,
    out: &mut Vec<RenderRow>,
) {
    if items.is_empty() {
        return;
    }
    out.push(RenderRow {
        entry_id,
        spans: vec![Span::styled("◈ 任务计划", theme::side_title())],
        links: Vec::new(),
        action: RowAction::None,
        copy_meta: RowCopyMeta::default(),
    });
    for item in items {
        let (mark, style) = match item.status.as_str() {
            "completed" | "done" => ("☑ ", theme::good()),
            "in_progress" => ("◐ ", theme::accent()),
            _ => ("☐ ", theme::dim()),
        };
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::styled("  ", Style::new()),
                Span::styled(mark, style),
                Span::styled(
                    linebreak::clip(&item.content, width.saturating_sub(4)),
                    theme::text(),
                ),
            ],
            links: Vec::new(),
            action: RowAction::None,
            copy_meta: RowCopyMeta::default(),
        });
    }
}

fn build_notice(entry: &Entry, is_error: bool, width: u16, out: &mut Vec<RenderRow>) {
    let style = if is_error { theme::error() } else { theme::dim() };
    let prefix = if is_error { "✕ " } else { "• " };
    let inner_w = usize::from(width.saturating_sub(2)).max(1);
    for (i, raw) in entry.text.lines().enumerate() {
        let cells: Vec<&str> = raw.graphemes(true).collect();
        for (j, r) in linebreak::lines(&cells, inner_w).into_iter().enumerate() {
            let lead = if i == 0 && j == 0 { prefix } else { "  " };
            out.push(RenderRow {
                entry_id: entry.id,
                spans: vec![
                    Span::styled(lead, style),
                    Span::styled(cells[r].concat(), style),
                ],
                links: Vec::new(),
                action: RowAction::None,
                copy_meta: RowCopyMeta::default(),
            });
        }
    }
}

fn blank_row(entry_id: u64) -> RenderRow {
    RenderRow {
        entry_id,
        spans: Vec::new(),
        links: Vec::new(),
        action: RowAction::None,
        copy_meta: RowCopyMeta::default(),
    }
}

pub fn row_to_line(row: &RenderRow) -> Line<'static> {
    Line::from(row.spans.clone())
}
