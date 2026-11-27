//! 主视图顶部黏性用户消息条：当上条用户消息滚出视口上方时，单行固定展示并支持点击跳转回完整消息。

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::linebreak;
use crate::theme;
use crate::transcript::{EntryKind, Transcript};

use super::HoverTarget;

/// 顶部黏性用户消息条的鼠标命中热区。
#[derive(Default, Debug, Clone)]
pub struct StickyPromptHit {
    pub area: Rect,
    pub entry_id: Option<u64>,
}

/// 解析当前视口上方最近一条已滚出的用户消息（直接按 `entry_start < scroll` 判定，避免回合分隔空行导致闪烁消失）。
///
/// 参数:
/// - `transcript`: 当前会话记录
/// - `entry_starts`: 各条目在正文渲染行中的起始行号映射
/// - `scroll`: 当前视口顶部行偏移
///
/// 返回:
/// - 存在已滚出视口顶部的用户消息时返回 `Some((entry_id, 用户消息原文))`，否则返回 `None`
pub(super) fn resolve<'a>(
    transcript: &'a Transcript,
    entry_starts: &HashMap<u64, usize>,
    scroll: usize,
) -> Option<(u64, &'a str)> {
    if scroll == 0 {
        return None;
    }
    // 取最后一条起始行严格位于视口顶部之上且文本非空的用户消息
    for entry in transcript.entries.iter().rev() {
        if !matches!(entry.kind, EntryKind::User { .. }) {
            continue;
        }
        let Some(&start) = entry_starts.get(&entry.id) else {
            continue;
        };
        if start < scroll {
            let trimmed = entry.text.trim();
            if !trimmed.is_empty() {
                return Some((entry.id, trimmed));
            }
        }
    }
    None
}

/// 【主视图】【顶部黏性用户条】在主栏顶部单行渲染滚出视口的上条用户消息，悬停仅高亮前景，点击跳转定位。
///
/// 参数:
/// - `frame`: 当前渲染帧
/// - `area`: 黏性条所在矩形（单行）
/// - `entry_id`: 关联用户消息条目 ID
/// - `raw_text`: 用户消息原始文本
/// - `hover`: 当前鼠标悬停目标
///
/// 返回:
/// - 用于悬停与点击跳转的 `StickyPromptHit`
pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    entry_id: u64,
    raw_text: &str,
    hover: &HoverTarget,
) -> StickyPromptHit {
    if area.width < 12 || area.height == 0 {
        return StickyPromptHit::default();
    }

    // 1. 压缩多行用户消息为紧凑单行摘要
    let one_line: String = raw_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.is_empty() {
        return StickyPromptHit::default();
    }

    let is_hover = matches!(hover, HoverTarget::StickyUserPrompt(id) if *id == entry_id);
    let bar_style = if is_hover {
        theme::accent_bold()
    } else {
        theme::user_bar()
    };
    let text_style = if is_hover {
        theme::accent_bold()
    } else {
        theme::dim()
    };
    let hint_style = if is_hover {
        theme::accent_bold()
    } else {
        theme::faint()
    };

    let right_hint = "↑ 跳转";
    let right_w = right_hint.width();
    let total_w = usize::from(area.width);
    let show_right = total_w >= right_w + 16;
    let text_room = if show_right {
        total_w.saturating_sub(2 + right_w + 2).max(4)
    } else {
        total_w.saturating_sub(2).max(4)
    };
    let clipped = linebreak::clip(&one_line, text_room as u16);

    let mut spans = vec![
        Span::styled("┃ ", bar_style),
        Span::styled(clipped.clone(), text_style),
    ];
    if show_right {
        let used = 2 + clipped.width();
        let gap = total_w.saturating_sub(used + right_w);
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled(right_hint, hint_style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    StickyPromptHit {
        area,
        entry_id: Some(entry_id),
    }
}
