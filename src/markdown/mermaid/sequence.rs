//! Mermaid 时序图（`sequenceDiagram`）解析与 Unicode 生命线渲染器。
//! 支持 `participant`/`actor` 别名、实线/虚线/终止箭头、自调用循环、Note 与 `alt`/`else`/`opt`/`loop` 分组框。

use std::collections::HashMap;

use unicode_width::UnicodeWidthStr;

use super::canvas::{Canvas, clip_width};
use crate::markdown::inline::Folded;
use crate::theme;

#[derive(Debug, Clone)]
struct Participant {
    label: String,
    is_actor: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrowKind {
    Solid,
    Dashed,
    CrossSolid,
    CrossDashed,
}

#[derive(Debug, Clone)]
enum SeqEvent {
    Message {
        from: usize,
        to: usize,
        text: String,
        kind: ArrowKind,
    },
    Note {
        text: String,
    },
    BlockHeader {
        keyword: String,
        title: String,
    },
    BlockEnd,
}

struct SequenceDiagram {
    participants: Vec<Participant>,
    events: Vec<SeqEvent>,
}

/// 尝试将 Mermaid `sequenceDiagram` 源码渲染为 Unicode 框线行集合。
///
/// 参数:
/// - `source`: 完整 Mermaid 源码
/// - `width`: 可用内容列宽
///
/// 返回:
/// - 成功渲染时返回 `Some(Vec<Folded>)`，语法不匹配或宽度极端不足时返回 `None`
pub fn render(source: &str, width: usize) -> Option<Vec<Folded>> {
    let diagram = parse(source)?;
    if diagram.participants.is_empty() || width < 20 {
        return None;
    }

    let count = diagram.participants.len();
    let min_col_w = 8usize;
    if count * min_col_w > width {
        return None;
    }

    // 1. 计算各参与者列中心坐标与顶部方框宽度
    let col_slot = width / count;
    let box_w_cap = col_slot.saturating_sub(1).max(6);
    let mut box_widths = Vec::with_capacity(count);
    let mut centers = Vec::with_capacity(count);
    for (idx, p) in diagram.participants.iter().enumerate() {
        let natural = p.label.width() + 4;
        let bw = natural.clamp(6, box_w_cap);
        box_widths.push(bw);
        centers.push(idx * col_slot + col_slot / 2);
    }

    // 2. 估算总高度并分配画布
    let mut total_h = 4usize; // 顶部盒子 (3) + 1 空生命线
    for ev in &diagram.events {
        total_h += match ev {
            SeqEvent::Message { from, to, .. } if from == to => 4,
            SeqEvent::Message { .. } => 3,
            SeqEvent::Note { .. } => 2,
            SeqEvent::BlockHeader { .. } => 2,
            SeqEvent::BlockEnd => 1,
        };
    }
    total_h += 3; // 底部参与者盒子

    let mut canvas = Canvas::new(width, total_h);

    // 3. 绘制贯穿全图的参与者生命线
    let lifeline_top = 3usize;
    let lifeline_bottom = total_h.saturating_sub(4);
    for &cx in &centers {
        canvas.draw_vline(cx, lifeline_top, lifeline_bottom, true, theme::faint());
    }

    // 4. 绘制顶部与底部参与者方框
    let bottom_box_y = total_h.saturating_sub(3);
    for (idx, p) in diagram.participants.iter().enumerate() {
        let bw = box_widths[idx];
        let x = centers[idx].saturating_sub(bw / 2);
        let style = if p.is_actor {
            theme::good()
        } else {
            theme::accent()
        };
        canvas.draw_box(x, 0, bw, 3, p.is_actor, style);
        canvas.put_centered(x + 2, 1, bw.saturating_sub(4), &p.label, theme::md_bold());

        canvas.draw_box(x, bottom_box_y, bw, 3, p.is_actor, style);
        canvas.put_centered(
            x + 2,
            bottom_box_y + 1,
            bw.saturating_sub(4),
            &p.label,
            theme::dim(),
        );
    }

    // 5. 按时间顺序绘制消息箭头、Note 与分组条
    let mut y = 4usize;
    let left_edge = centers.first().copied().unwrap_or(0).saturating_sub(2);
    let right_edge = (centers.last().copied().unwrap_or(width - 1) + 2).min(width - 1);

    for ev in &diagram.events {
        match ev {
            SeqEvent::Message {
                from,
                to,
                text,
                kind,
            } => {
                let dashed = matches!(kind, ArrowKind::Dashed | ArrowKind::CrossDashed);
                let cross = matches!(kind, ArrowKind::CrossSolid | ArrowKind::CrossDashed);
                let cx1 = centers[*from];
                let cx2 = centers[*to];

                if from == to {
                    // 自调用环回箭头
                    let loop_w = 4usize.min(width.saturating_sub(cx1 + 2));
                    let right_x = cx1 + loop_w;
                    let lbl_x = right_x + 2;
                    if lbl_x < width {
                        let clipped = clip_width(text, width - lbl_x);
                        canvas.put_str(lbl_x, y + 1, &clipped, theme::text());
                    }
                    canvas.draw_hline(y, cx1 + 1, right_x, dashed, theme::dim());
                    canvas.put_char(right_x, y, '┐', theme::dim());
                    canvas.put_char(right_x, y + 1, '│', theme::dim());
                    canvas.draw_hline(y + 2, cx1 + 1, right_x, dashed, theme::dim());
                    canvas.put_char(right_x, y + 2, '┘', theme::dim());
                    canvas.put_char(cx1 + 1, y + 2, if cross { '×' } else { '◀' }, theme::accent());
                    y += 4;
                } else {
                    let (lo, hi, to_right) = if cx1 < cx2 {
                        (cx1, cx2, true)
                    } else {
                        (cx2, cx1, false)
                    };
                    let span_w = hi.saturating_sub(lo).saturating_sub(1).max(1);
                    canvas.put_centered(lo + 1, y, span_w, text, theme::text());
                    let line_y = y + 1;
                    if hi > lo + 1 {
                        canvas.draw_hline(line_y, lo + 1, hi - 1, dashed, theme::dim());
                    }
                    let head = if cross {
                        '×'
                    } else if to_right {
                        '▶'
                    } else {
                        '◀'
                    };
                    let head_x = if to_right {
                        hi.saturating_sub(1)
                    } else {
                        lo + 1
                    };
                    canvas.put_char(head_x, line_y, head, theme::accent());
                    y += 3;
                }
            }
            SeqEvent::Note { text } => {
                let tag = format!("Note: {text}");
                let clipped = clip_width(&tag, right_edge.saturating_sub(left_edge) + 1);
                canvas.put_centered(
                    left_edge,
                    y,
                    right_edge.saturating_sub(left_edge) + 1,
                    &clipped,
                    theme::thought(),
                );
                y += 2;
            }
            SeqEvent::BlockHeader { keyword, title } => {
                let header = if title.is_empty() {
                    format!("── {keyword} ──")
                } else {
                    format!("── {keyword}: {title} ──")
                };
                let span_w = right_edge.saturating_sub(left_edge) + 1;
                canvas.draw_hline(y, left_edge, right_edge, true, theme::faint());
                canvas.put_str(left_edge, y, &clip_width(&header, span_w), theme::warn());
                y += 2;
            }
            SeqEvent::BlockEnd => {
                canvas.draw_hline(y, left_edge, right_edge, true, theme::faint());
                y += 1;
            }
        }
    }

    Some(canvas.into_folded())
}

fn parse(source: &str) -> Option<SequenceDiagram> {
    let mut lines = source
        .lines()
        .map(|l| l.find("%%").map_or(l, |i| &l[..i]))
        .map(str::trim)
        .filter(|l| !l.is_empty());

    let first = lines.next()?;
    if !first.eq_ignore_ascii_case("sequenceDiagram") {
        return None;
    }

    let mut participants: Vec<Participant> = Vec::new();
    let mut id_map: HashMap<String, usize> = HashMap::new();
    let mut events: Vec<SeqEvent> = Vec::new();

    let mut ensure_participant = |id: &str, label: Option<String>, is_actor: bool| -> usize {
        if let Some(&idx) = id_map.get(id) {
            if let Some(lbl) = label {
                participants[idx].label = lbl;
            }
            if is_actor {
                participants[idx].is_actor = true;
            }
            return idx;
        }
        let idx = participants.len();
        participants.push(Participant {
            label: label.unwrap_or_else(|| id.to_string()),
            is_actor,
        });
        id_map.insert(id.to_string(), idx);
        idx
    };

    for line in lines {
        let lower = line.to_ascii_lowercase();
        if lower == "autonumber"
            || lower.starts_with("activate ")
            || lower.starts_with("deactivate ")
            || lower.starts_with("title ")
        {
            continue;
        }
        if let Some(rest) = strip_prefix_ci(line, "participant ") {
            let (id, label) = parse_participant_decl(rest);
            ensure_participant(id, label, false);
            continue;
        }
        if let Some(rest) = strip_prefix_ci(line, "actor ") {
            let (id, label) = parse_participant_decl(rest);
            ensure_participant(id, label, true);
            continue;
        }
        if lower == "end" {
            events.push(SeqEvent::BlockEnd);
            continue;
        }
        if let Some((kw, title)) = parse_block_header(line) {
            events.push(SeqEvent::BlockHeader {
                keyword: kw,
                title,
            });
            continue;
        }
        if lower.starts_with("note ") {
            let note_text = line
                .split_once(':')
                .map(|(_, rhs)| rhs.trim())
                .unwrap_or(line);
            events.push(SeqEvent::Note {
                text: note_text.to_string(),
            });
            continue;
        }
        if let Some((from_id, to_id, kind, msg)) = parse_message(line) {
            let from = ensure_participant(from_id, None, false);
            let to = ensure_participant(to_id, None, false);
            events.push(SeqEvent::Message {
                from,
                to,
                text: msg,
                kind,
            });
        }
    }

    (!participants.is_empty()).then_some(SequenceDiagram {
        participants,
        events,
    })
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

fn parse_participant_decl(rest: &str) -> (&str, Option<String>) {
    let trimmed = rest.trim();
    if let Some(idx) = trimmed.find(" as ") {
        let id = trimmed[..idx].trim();
        let alias = trimmed[idx + 4..].trim().trim_matches('"').to_string();
        (id, Some(alias))
    } else {
        (trimmed, None)
    }
}

fn parse_block_header(line: &str) -> Option<(String, String)> {
    const KEYWORDS: &[&str] = &["alt", "else", "opt", "loop", "par", "and", "critical", "break"];
    for &kw in KEYWORDS {
        if line.eq_ignore_ascii_case(kw) {
            return Some((kw.to_string(), String::new()));
        }
        let prefix = format!("{kw} ");
        if let Some(rest) = strip_prefix_ci(line, &prefix) {
            return Some((kw.to_string(), rest.trim().to_string()));
        }
    }
    None
}

fn parse_message(line: &str) -> Option<(&str, &str, ArrowKind, String)> {
    const ARROWS: &[(&str, ArrowKind)] = &[
        ("-->>", ArrowKind::Dashed),
        ("->>", ArrowKind::Solid),
        ("--x", ArrowKind::CrossDashed),
        ("-x", ArrowKind::CrossSolid),
        ("--)", ArrowKind::Dashed),
        ("-)", ArrowKind::Solid),
        ("-->", ArrowKind::Dashed),
        ("->", ArrowKind::Solid),
    ];
    for &(tok, kind) in ARROWS {
        if let Some(pos) = line.find(tok) {
            let from = line[..pos].trim();
            let rhs = &line[pos + tok.len()..];
            let (raw_to, text) = match rhs.split_once(':') {
                Some((t, m)) => (t.trim(), m.trim().to_string()),
                None => (rhs.trim(), String::new()),
            };
            let to = raw_to.trim_start_matches(['+', '-']).trim();
            if !from.is_empty() && !to.is_empty() {
                return Some((from, to, kind, text));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::inline::plain;

    #[test]
    fn sequence_diagram_renders_lifelines_and_messages() {
        let src = "\
sequenceDiagram
    participant U as 用户
    participant A as ACP后端
    U->>A: 发送提问
    A-->>U: 流式响应
";
        let rows = render(src, 64).expect("should render sequence diagram");
        let joined = rows.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("用户"));
        assert!(joined.contains("ACP后端"));
        assert!(joined.contains("发送提问"));
        assert!(joined.contains("流式响应"));
        assert!(joined.contains('▶'));
        assert!(joined.contains('◀'));
    }
}
