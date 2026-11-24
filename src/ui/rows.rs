//! 正文时间线行构建器与 Markdown 宽度缓存。

use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::acp::ToolDiff;
use crate::diff;
use crate::linebreak;
use crate::markdown::{self, MdLine};
use crate::theme::{self, ThemePreset};
use crate::transcript::{Entry, EntryKind, Transcript, format_duration, format_short};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    pub clickable_entry: bool,
    pub copy_meta: RowCopyMeta,
}

#[derive(Hash, PartialEq, Eq)]
struct CacheKey {
    entry_id: u64,
    text_len: usize,
    width: u16,
    preset: u8,
    paint_bg: bool,
}

pub struct MdCache {
    entries: HashMap<CacheKey, Vec<MdLine>>,
}

impl MdCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn render(&mut self, entry_id: u64, text: &str, width: u16) -> &[MdLine] {
        let key = CacheKey {
            entry_id,
            text_len: text.len(),
            width,
            preset: preset_id(theme::current_preset()),
            paint_bg: theme::paint_bg(),
        };
        if self.entries.len() > 256 && !self.entries.contains_key(&key) {
            self.entries.clear();
        }
        self.entries
            .entry(key)
            .or_insert_with(|| markdown::render(text, width))
    }
}

fn preset_id(p: ThemePreset) -> u8 {
    match p {
        ThemePreset::MiyuLavender => 0,
        ThemePreset::MiyuNight => 1,
        ThemePreset::MiyuForest => 2,
        ThemePreset::MiyuDay => 3,
        ThemePreset::Terminal => 4,
    }
}

pub struct BuiltBody {
    pub rows: Vec<RenderRow>,
    pub entry_starts: HashMap<u64, usize>,
}

pub fn build_body(
    transcript: &Transcript,
    width: u16,
    tick: usize,
    cache: &mut MdCache,
) -> BuiltBody {
    let width = width.max(16);
    let mut rows = Vec::new();
    let mut entry_starts = HashMap::new();

    for (idx, entry) in transcript.entries.iter().enumerate() {
        if idx > 0 {
            let need_gap = match (&transcript.entries[idx - 1].kind, &entry.kind) {
                (EntryKind::Tool { .. }, EntryKind::Tool { .. }) => false,
                _ => true,
            };
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
            } => build_thought(entry, *started, *elapsed, *expanded, width, tick, &mut rows),
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
        clickable_entry: false,
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
                clickable_entry: false,
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
        clickable_entry: false,
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

fn build_thought(
    entry: &Entry,
    started: std::time::Instant,
    elapsed: Option<std::time::Duration>,
    expanded: bool,
    width: u16,
    tick: usize,
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
    let hint = if expanded { " ▾" } else { " ▸" };
    let header_style = if busy { theme::accent() } else { theme::dim() };

    out.push(RenderRow {
        entry_id: entry.id,
        spans: vec![
            Span::styled(linebreak::clip(&header, width.saturating_sub(3)), header_style),
            Span::styled(hint, theme::faint()),
        ],
        links: Vec::new(),
        clickable_entry: true,
        copy_meta: RowCopyMeta::default(),
    });

    let trimmed = entry.text.trim();
    if trimmed.is_empty() {
        return;
    }

    let inner_w = usize::from(width.saturating_sub(2)).max(1);
    let mut wrapped: Vec<String> = Vec::new();
    for raw_line in trimmed.lines() {
        let cells: Vec<&str> = raw_line.graphemes(true).collect();
        for range in linebreak::lines(&cells, inner_w) {
            wrapped.push(cells[range].concat());
        }
    }

    let preview_limit = 3;
    let shown: &[String] = if expanded || wrapped.len() <= preview_limit {
        &wrapped
    } else {
        &wrapped[wrapped.len() - preview_limit..]
    };

    if !expanded && wrapped.len() > preview_limit {
        out.push(RenderRow {
            entry_id: entry.id,
            spans: vec![
                Span::styled("⋮ ", theme::faint()),
                Span::styled(
                    format!("已折叠前 {} 行（点击或 Ctrl+O 展开）", wrapped.len() - preview_limit),
                    theme::faint(),
                ),
            ],
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: true,
                join_previous: false,
            },
        });
    }

    for line in shown {
        out.push(RenderRow {
            entry_id: entry.id,
            spans: vec![
                Span::styled("│ ", theme::faint()),
                Span::styled(line.clone(), theme::thought()),
            ],
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: false,
                join_previous: false,
            },
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn build_tool(
    entry_id: u64,
    name: &str,
    title: &str,
    kind: &str,
    status: &str,
    raw_input: Option<&Value>,
    output: &str,
    diffs: &[ToolDiff],
    expanded: bool,
    width: u16,
    tick: usize,
    out: &mut Vec<RenderRow>,
) {
    let (icon, icon_style) = match status {
        "completed" => ("✓", theme::good()),
        "failed" | "error" | "cancelled" => ("✕", theme::error()),
        "pending" => ("◈", theme::warn()),
        _ => (SPINNER[tick % SPINNER.len()], theme::accent()),
    };
    let kind_label = match kind {
        "read" => "读取",
        "edit" => "编辑",
        "execute" => "执行",
        "search" => "搜索",
        "fetch" => "网络",
        _ => name,
    };
    let clean_title = title.replace('\n', " ");
    let is_read_tool = is_read_tool_call(name, kind);
    let read_path = if is_read_tool {
        extract_read_path(&clean_title, raw_input)
    } else {
        None
    };
    let read_range_badge = if is_read_tool {
        read_header_range_badge(output, raw_input)
    } else {
        None
    };

    let summary = if is_read_tool {
        let display_name = read_path
            .as_deref()
            .map(file_basename)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if clean_title.eq_ignore_ascii_case(name) || clean_title.is_empty() {
                    name.to_string()
                } else {
                    clean_title.clone()
                }
            });
        format!("{kind_label} · {display_name}")
    } else if clean_title.eq_ignore_ascii_case(name) || clean_title.is_empty() {
        format!("{kind_label} · {name}")
    } else {
        format!("{kind_label} · {clean_title}")
    };

    // 预计算每个 diff 的统计，方便标题行直接显示 `+N -M`
    let computed_diffs: Vec<(&ToolDiff, diff::ComputedDiff)> = diffs
        .iter()
        .map(|d| (d, diff::compute(&d.old_text, &d.new_text)))
        .collect();
    let total_added: usize = computed_diffs.iter().map(|(_, c)| c.added).sum();
    let total_removed: usize = computed_diffs.iter().map(|(_, c)| c.removed).sum();

    let has_detail = !computed_diffs.is_empty()
        || read_path.is_some()
        || raw_input.is_some()
        || !output.trim().is_empty();
    let fold_mark = if !has_detail {
        ""
    } else if expanded {
        " ▾"
    } else {
        " ▸"
    };
    let text_style = if status == "failed" || status == "error" {
        theme::error()
    } else {
        theme::dim()
    };

    let mut stat_spans = Vec::new();
    let mut stat_w = 0usize;
    if (total_added > 0 || total_removed > 0) && status != "failed" && status != "error" {
        let add_s = format!("+{total_added}");
        let rem_s = format!("-{total_removed}");
        stat_w = 3 + add_s.len() + 1 + rem_s.len();
        stat_spans.push(Span::styled(" · ", theme::faint()));
        stat_spans.push(Span::styled(add_s, theme::added()));
        stat_spans.push(Span::raw(" "));
        stat_spans.push(Span::styled(rem_s, theme::removed()));
    } else if let Some(range) = &read_range_badge {
        if status != "failed" && status != "error" {
            stat_w = 3 + range.width();
            stat_spans.push(Span::styled(" · ", theme::faint()));
            stat_spans.push(Span::styled(range.clone(), theme::faint()));
        }
    }

    let summary_room = usize::from(width)
        .saturating_sub(5 + stat_w)
        .max(8) as u16;
    let exec_command = extract_command_text(name, kind, title, raw_input);
    let mut header_spans = if let Some(cmd) = exec_command.as_deref() {
        let one_line_cmd = cmd.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ; ");
        let verb_style = if status == "failed" || status == "error" {
            theme::error()
        } else {
            theme::model()
        };
        let mut spans = vec![
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(kind_label.to_string(), verb_style),
            Span::styled(" · ", theme::faint()),
            Span::styled("$ ", theme::picked()),
        ];
        let used_prefix = 2 + kind_label.width() + 3 + 2;
        let cmd_room = usize::from(width)
            .saturating_sub(used_prefix + stat_w + fold_mark.width())
            .max(8);
        spans.extend(clip_highlighted_shell(&one_line_cmd, cmd_room));
        spans
    } else {
        vec![
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(linebreak::clip(&summary, summary_room), text_style),
        ]
    };
    header_spans.extend(stat_spans);
    header_spans.push(Span::styled(fold_mark, theme::faint()));

    out.push(RenderRow {
        entry_id,
        spans: header_spans,
        links: Vec::new(),
        clickable_entry: has_detail,
        copy_meta: RowCopyMeta::default(),
    });

    if !expanded {
        return;
    }

    let inner_w = usize::from(width.saturating_sub(2)).max(1);

    // 1. 若有结构化 Diff，展开后展示 `sai` 风格语法高亮 + 行内差异强调 Diff
    if !computed_diffs.is_empty() {
        let multi_file = computed_diffs.len() > 1;
        for (idx, (td, computed)) in computed_diffs.iter().enumerate() {
            if idx > 0 {
                out.push(RenderRow {
                    entry_id,
                    spans: Vec::new(),
                    links: Vec::new(),
                    clickable_entry: true,
                    copy_meta: RowCopyMeta::default(),
                });
            }
            if !td.path.is_empty() {
                let path_shown = linebreak::clip(&td.path, inner_w.saturating_sub(14) as u16);
                let path_w = u16::try_from(path_shown.width()).unwrap_or(0);
                let mut sub = vec![
                    Span::styled(" • ", theme::faint()),
                    Span::styled(path_shown, theme::md_url()),
                ];
                if multi_file {
                    sub.push(Span::styled(" (", theme::faint()));
                    sub.push(Span::styled(format!("+{}", computed.added), theme::added()));
                    sub.push(Span::raw(" "));
                    sub.push(Span::styled(format!("-{}", computed.removed), theme::removed()));
                    sub.push(Span::styled(")", theme::faint()));
                }
                out.push(RenderRow {
                    entry_id,
                    spans: sub,
                    links: vec![(3, 3 + path_w, td.path.clone())],
                    clickable_entry: true,
                    copy_meta: RowCopyMeta::default(),
                });
            }
            if computed.rows.is_empty() {
                out.push(RenderRow {
                    entry_id,
                    spans: vec![
                        Span::raw(" "),
                        Span::styled("(无文本变更)", theme::faint()),
                    ],
                    links: Vec::new(),
                    clickable_entry: true,
                    copy_meta: RowCopyMeta::default(),
                });
            } else {
                let (diff_lines, omitted) = diff::render_lines(computed, &td.path, width, true);
                for spans in diff_lines {
                    out.push(RenderRow {
                        entry_id,
                        spans,
                        links: Vec::new(),
                        clickable_entry: true,
                        copy_meta: RowCopyMeta::default(),
                    });
                }
                if omitted > 0 {
                    out.push(RenderRow {
                        entry_id,
                        spans: vec![
                            Span::raw(" "),
                            Span::styled(format!("⋮ 已截断剩余 {omitted} 行 diff"), theme::faint()),
                        ],
                        links: Vec::new(),
                        clickable_entry: true,
                        copy_meta: RowCopyMeta::default(),
                    });
                }
            }
        }
        let trimmed_out = output.trim();
        if !trimmed_out.is_empty() {
            push_detail_lines(entry_id, trimmed_out, theme::dim(), inner_w, 12, out);
        }
        return;
    }

    // 2. 文件读取工具：与 Diff 一致的点击展开逻辑 + 参考 `sai/read_file.rs` 的带行号语法高亮代码预览
    if is_read_tool {
        let path_str = read_path.as_deref().unwrap_or("");
        if !path_str.is_empty() {
            let path_shown = linebreak::clip(path_str, inner_w.saturating_sub(4) as u16);
            let path_w = u16::try_from(path_shown.width()).unwrap_or(0);
            out.push(RenderRow {
                entry_id,
                spans: vec![
                    Span::styled(" • ", theme::faint()),
                    Span::styled(path_shown, theme::md_url()),
                ],
                links: vec![(3, 3 + path_w, path_str.to_string())],
                clickable_entry: true,
                copy_meta: RowCopyMeta::default(),
            });
        }
        if let Some(preview) = build_read_preview(output, path_str, raw_input) {
            push_read_preview_rows(entry_id, &preview, width, 160, out);
        } else if !output.trim().is_empty() {
            push_detail_lines(entry_id, output.trim(), theme::dim(), inner_w, 40, out);
        } else {
            out.push(RenderRow {
                entry_id,
                spans: vec![
                    Span::raw(" "),
                    Span::styled("(无预览内容)", theme::faint()),
                ],
                links: Vec::new(),
                clickable_entry: true,
                copy_meta: RowCopyMeta::default(),
            });
        }
        return;
    }

    // 3. 普通工具（命令执行、搜索等）
    if let Some(cmd) = exec_command {
        push_highlighted_command_lines(entry_id, &cmd, inner_w, 8, out);
    } else if let Some(input) = raw_input {
        if let Some(path) = input["Path"]
            .as_str()
            .or_else(|| input["path"].as_str())
        {
            push_detail_lines(entry_id, path, theme::md_url(), inner_w, 2, out);
        }
    }
    let trimmed_out = output.trim();
    if !trimmed_out.is_empty() {
        push_detail_lines(entry_id, trimmed_out, theme::dim(), inner_w, 24, out);
    }
}

struct ReadPreviewLine {
    line_no: usize,
    text: String,
}

struct ReadPreview {
    path: String,
    lines: Vec<ReadPreviewLine>,
    notices: Vec<String>,
}

fn is_read_tool_call(name: &str, kind: &str) -> bool {
    kind == "read"
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "read_file" | "read" | "view_file" | "cat"
        )
}

fn file_basename(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    trimmed
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

fn extract_read_path(title: &str, raw_input: Option<&Value>) -> Option<String> {
    if let Some(inp) = raw_input {
        for key in ["Path", "path", "AbsolutePath", "file_path", "filePath", "target_file"] {
            if let Some(p) = inp[key].as_str() {
                let decoded = crate::acp::decode_acp_cwd(p.trim());
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }
    let mut cleaned = title.trim();
    for prefix in ["Read ", "Reading ", "File ", "View "] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            cleaned = rest.trim();
            break;
        }
    }
    // 去掉标题尾部的行号区间描述（如 `foo.rs (1-50)`）
    let cleaned = cleaned
        .split_once(" (")
        .map_or(cleaned, |(head, _)| head.trim());
    let decoded = crate::acp::decode_acp_cwd(cleaned);
    if !decoded.is_empty()
        && (decoded.contains('\\') || decoded.contains('/') || decoded.contains('.'))
    {
        Some(decoded)
    } else {
        None
    }
}

fn extract_requested_read_range(raw_input: Option<&Value>) -> (Option<usize>, Option<usize>) {
    let Some(inp) = raw_input else {
        return (None, None);
    };
    let get_usize = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| inp[*k].as_u64())
            .map(|v| v as usize)
            .filter(|&v| v > 0)
    };
    let start = get_usize(&["StartLine", "start_line", "startLine", "offset"]);
    let end = get_usize(&["EndLine", "end_line", "endLine"]).or_else(|| {
        let limit = get_usize(&["limit", "Limit", "max_lines"])?;
        Some(start.unwrap_or(1).saturating_add(limit.saturating_sub(1)))
    });
    (start, end)
}

fn split_read_numbered_line(line: &str) -> Option<(usize, &str)> {
    let trimmed_start = line.trim_start_matches(' ');
    let (num_str, rest) = trimmed_start
        .split_once('\t')
        .or_else(|| trimmed_start.split_once(": "))
        .or_else(|| trimmed_start.split_once(" | "))?;
    let num_str = num_str.trim();
    if num_str.is_empty() || !num_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let no = num_str.parse::<usize>().ok()?;
    Some((no, rest))
}

fn parse_read_output_only(output: &str, path: &str, raw_input: Option<&Value>) -> Option<ReadPreview> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. 兼容 `sai` text-page JSON 格式
    if trimmed.starts_with('{') {
        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            if let Some(content) = val["content"].as_str() {
                let p = val["path"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(crate::acp::decode_acp_cwd)
                    .unwrap_or_else(|| path.to_string());
                let offset = val["offset"].as_u64().unwrap_or(1).max(1) as usize;
                let mut lines = Vec::new();
                for (i, raw_line) in content.lines().enumerate() {
                    if let Some((no, rest)) = split_read_numbered_line(raw_line) {
                        lines.push(ReadPreviewLine {
                            line_no: no,
                            text: rest.to_string(),
                        });
                    } else {
                        lines.push(ReadPreviewLine {
                            line_no: offset + i,
                            text: raw_line.to_string(),
                        });
                    }
                }
                let mut notices = Vec::new();
                if val["truncated"].as_bool() == Some(true) {
                    if let Some(next) = val["next"].as_u64() {
                        notices.push(format!("后续内容起始行：{next}"));
                    }
                }
                if !lines.is_empty() {
                    return Some(ReadPreview {
                        path: p,
                        lines,
                        notices,
                    });
                }
            }
        }
    }

    // 2. 识别带行号文本（`N\tcode` / `N: code` / `N | code`，允许首行 `Showing lines ...` 或尾部提醒）
    let mut numbered = Vec::new();
    let mut notices = Vec::new();
    let mut saw_trailing_notice = false;
    let mut valid_numbered = true;
    for (idx, line) in trimmed.lines().enumerate() {
        if let Some((no, rest)) = split_read_numbered_line(line) {
            if saw_trailing_notice {
                valid_numbered = false;
                break;
            }
            numbered.push(ReadPreviewLine {
                line_no: no,
                text: rest.to_string(),
            });
        } else if idx == 0 && (line.starts_with("Showing lines ") || line.starts_with("File: ")) {
            notices.push(line.to_string());
        } else if !numbered.is_empty() {
            saw_trailing_notice = true;
            if !line.trim().is_empty() {
                notices.push(line.to_string());
            }
        } else {
            valid_numbered = false;
            break;
        }
    }
    if valid_numbered && !numbered.is_empty() {
        return Some(ReadPreview {
            path: path.to_string(),
            lines: numbered,
            notices,
        });
    }

    // 3. 纯源码文本输出（如未带行号前缀）：按起始行号补齐行号并着色
    let (req_start, _) = extract_requested_read_range(raw_input);
    let base = req_start.unwrap_or(1);
    let lines: Vec<ReadPreviewLine> = output
        .lines()
        .enumerate()
        .map(|(i, l)| ReadPreviewLine {
            line_no: base + i,
            text: l.to_string(),
        })
        .collect();
    (!lines.is_empty()).then(|| ReadPreview {
        path: path.to_string(),
        lines,
        notices: Vec::new(),
    })
}

fn read_header_range_badge(output: &str, raw_input: Option<&Value>) -> Option<String> {
    if let Some(preview) = parse_read_output_only(output, "", raw_input) {
        if let (Some(first), Some(last)) = (preview.lines.first(), preview.lines.last()) {
            return Some(if first.line_no == last.line_no {
                format!("L{}", first.line_no)
            } else {
                format!("{}–{}", first.line_no, last.line_no)
            });
        }
    }
    match extract_requested_read_range(raw_input) {
        (Some(s), Some(e)) if s == e => Some(format!("L{s}")),
        (Some(s), Some(e)) => Some(format!("{s}–{e}")),
        (Some(s), None) => Some(format!("L{s}+")),
        _ => None,
    }
}

/// 展开时构建读取预览：优先解析工具返回文本，若 ACP 后端未回传正文则按路径/区间安全回读本地文件兜底。
fn build_read_preview(output: &str, path: &str, raw_input: Option<&Value>) -> Option<ReadPreview> {
    if let Some(parsed) = parse_read_output_only(output, path, raw_input) {
        return Some(parsed);
    }
    if path.is_empty() {
        return None;
    }
    let fs_path = std::path::Path::new(path);
    let meta = std::fs::metadata(fs_path).ok()?;
    if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
        return None;
    }
    let content = std::fs::read_to_string(fs_path).ok()?;
    let all_lines: Vec<&str> = content.lines().collect();
    if all_lines.is_empty() {
        return Some(ReadPreview {
            path: path.to_string(),
            lines: Vec::new(),
            notices: vec!["(空文件)".to_string()],
        });
    }
    let (req_start, req_end) = extract_requested_read_range(raw_input);
    let start = req_start.unwrap_or(1).clamp(1, all_lines.len());
    let default_end = start.saturating_add(119);
    let end = req_end
        .unwrap_or(default_end)
        .min(start.saturating_add(159))
        .min(all_lines.len());
    let lines = (start..=end)
        .map(|no| ReadPreviewLine {
            line_no: no,
            text: all_lines[no - 1].to_string(),
        })
        .collect();
    let mut notices = Vec::new();
    if end < all_lines.len() {
        notices.push(format!("共 {} 行（当前预览 {start}–{end} 行）", all_lines.len()));
    }
    Some(ReadPreview {
        path: path.to_string(),
        lines,
        notices,
    })
}

/// 参考 `sai/read_file.rs` 与 `diff.rs` 渲染带中性灰行号 + 文件语言语法高亮的读取预览。
fn push_read_preview_rows(
    entry_id: u64,
    preview: &ReadPreview,
    width: u16,
    max_visual_lines: usize,
    out: &mut Vec<RenderRow>,
) {
    let max_no = preview
        .lines
        .iter()
        .map(|l| l.line_no)
        .max()
        .unwrap_or(1);
    let num_w = max_no.to_string().len().max(3);
    // 左内收 1 列 + 行号 num_w + 2 空格间隔
    let prefix_w = 1 + num_w + 2;
    let code_w = usize::from(width).saturating_sub(prefix_w + 1).max(8);

    let lang = diff::language_from_path(std::path::Path::new(&preview.path))
        .and_then(markdown::code::find_language);
    let mut hl = markdown::code::Highlighter::new(lang);

    let mut emitted = 0usize;
    let mut omitted = 0usize;
    for line in &preview.lines {
        let expanded = linebreak::expand_tabs(&line.text, 4);
        let pieces = hl.line(&expanded);
        let pieces = if pieces.is_empty() {
            vec![markdown::inline::Piece::new("", theme::text())]
        } else {
            pieces
        };
        for (wrap_idx, fl) in markdown::inline::fold(&pieces, code_w as u16)
            .into_iter()
            .enumerate()
        {
            emitted += 1;
            if emitted > max_visual_lines {
                omitted += 1;
                continue;
            }
            let gutter = if wrap_idx == 0 {
                format!(" {:>num_w$}  ", line.line_no)
            } else {
                " ".repeat(prefix_w)
            };
            let mut spans = Vec::with_capacity(1 + fl.spans.len());
            spans.push(Span::styled(gutter, theme::faint()));
            spans.extend(fl.spans);
            out.push(RenderRow {
                entry_id,
                spans,
                links: Vec::new(),
                clickable_entry: true,
                copy_meta: RowCopyMeta {
                    content_left: u16::try_from(prefix_w).unwrap_or(u16::MAX),
                    decorative: false,
                    join_previous: wrap_idx > 0,
                },
            });
        }
    }
    if omitted > 0 {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::raw(" ".repeat(1 + num_w)),
                Span::styled(format!(" ⋮ 已省略剩余 {omitted} 行"), theme::faint()),
            ],
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: u16::try_from(prefix_w).unwrap_or(u16::MAX),
                decorative: true,
                join_previous: false,
            },
        });
    }
    for notice in &preview.notices {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::raw(" "),
                Span::styled(linebreak::clip(notice, width.saturating_sub(2)), theme::faint()),
            ],
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: 1,
                decorative: true,
                join_previous: false,
            },
        });
    }
}

fn extract_command_text(
    name: &str,
    kind: &str,
    title: &str,
    raw_input: Option<&Value>,
) -> Option<String> {
    if let Some(inp) = raw_input {
        if let Some(cmd) = inp["CommandLine"]
            .as_str()
            .or_else(|| inp["command"].as_str())
            .or_else(|| inp["cmd"].as_str())
        {
            let trimmed = cmd.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    let is_exec = kind == "execute"
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "run_command" | "execute" | "shell" | "bash" | "powershell" | "terminal"
        );
    if is_exec {
        let cleaned = title.trim().strip_prefix("$ ").unwrap_or(title.trim());
        if !cleaned.is_empty() && !cleaned.eq_ignore_ascii_case(name) {
            return Some(cleaned.to_string());
        }
    }
    None
}

/// 将单行 Shell 命令按指定最大显示宽度截断并保留语法着色。
fn clip_highlighted_shell(cmd: &str, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }
    let pieces = markdown::code::highlight_shell_line(cmd, None);
    let total_w: usize = pieces.iter().map(|p| p.text.width()).sum();
    if total_w <= max_width {
        return pieces
            .into_iter()
            .map(|p| Span::styled(p.text, p.style))
            .collect();
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out = Vec::new();
    'outer: for p in pieces {
        let mut buf = String::new();
        for g in p.text.graphemes(true) {
            let w = g.width();
            if used + w > budget {
                if !buf.is_empty() {
                    out.push(Span::styled(buf, p.style));
                }
                break 'outer;
            }
            buf.push_str(g);
            used += w;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, p.style));
        }
    }
    out.push(Span::styled("…", theme::faint()));
    out
}

/// 将展开后的命令明细按 `sai` 风格逐行做 Shell 语法高亮，续行对齐到命令正文列。
fn push_highlighted_command_lines(
    entry_id: u64,
    cmd: &str,
    inner_w: usize,
    max_lines: usize,
    out: &mut Vec<RenderRow>,
) {
    let lang = markdown::code::detect_shell_language(cmd);
    let body_w = inner_w.saturating_sub(2).max(4);
    let mut rendered_rows: Vec<Vec<Span<'static>>> = Vec::new();
    for (line_idx, raw_line) in cmd.lines().enumerate() {
        let expanded = linebreak::expand_tabs(raw_line, 4);
        let pieces = markdown::code::highlight_shell_line(&expanded, Some(lang));
        let folded = markdown::inline::fold(&pieces, body_w as u16);
        for (wrap_idx, fl) in folded.into_iter().enumerate() {
            let mut spans = vec![Span::styled("│ ", theme::faint())];
            if line_idx == 0 && wrap_idx == 0 {
                spans.push(Span::styled("$ ", theme::picked()));
            } else {
                spans.push(Span::raw("  "));
            }
            spans.extend(fl.spans);
            rendered_rows.push(spans);
        }
    }
    let omitted = rendered_rows.len().saturating_sub(max_lines);
    for spans in rendered_rows.into_iter().take(max_lines) {
        out.push(RenderRow {
            entry_id,
            spans,
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: 4,
                decorative: false,
                join_previous: false,
            },
        });
    }
    if omitted > 0 {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::styled("⋮ ", theme::faint()),
                Span::styled(format!("已省略 {omitted} 行命令"), theme::faint()),
            ],
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: true,
                join_previous: false,
            },
        });
    }
}

fn push_detail_lines(
    entry_id: u64,
    text: &str,
    style: Style,
    inner_w: usize,
    max_lines: usize,
    out: &mut Vec<RenderRow>,
) {
    let mut wrapped = Vec::new();
    for raw in text.lines() {
        let expanded = linebreak::expand_tabs(raw, 4);
        let cells: Vec<&str> = expanded.graphemes(true).collect();
        for r in linebreak::lines(&cells, inner_w) {
            wrapped.push(cells[r].concat());
        }
    }
    let omitted = wrapped.len().saturating_sub(max_lines);
    for line in wrapped.into_iter().take(max_lines) {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::styled("│ ", theme::faint()),
                Span::styled(line, style),
            ],
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: false,
                join_previous: false,
            },
        });
    }
    if omitted > 0 {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::styled("⋮ ", theme::faint()),
                Span::styled(format!("已省略 {omitted} 行"), theme::faint()),
            ],
            links: Vec::new(),
            clickable_entry: true,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: true,
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
            clickable_entry: false,
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
            clickable_entry: false,
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
        clickable_entry: false,
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
            clickable_entry: false,
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
                clickable_entry: false,
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
        clickable_entry: false,
        copy_meta: RowCopyMeta::default(),
    }
}

pub fn row_to_line(row: &RenderRow) -> Line<'static> {
    Line::from(row.spans.clone())
}
