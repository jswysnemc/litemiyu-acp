//! 正文各类折叠提示公用的渲染行。

use ratatui::text::Span;

use super::{RenderRow, RowAction, RowCopyMeta};
use crate::{theme, transcript::FoldSection};

/// 【正文】【折叠提示】按条目与分区生成命中目标；`omitted` 是实际隐藏的视觉行数。
pub(super) fn marker(entry_id: u64, section: FoldSection, omitted: usize) -> RenderRow {
    let position = if section == FoldSection::Thought {
        "前"
    } else {
        "剩余"
    };
    RenderRow {
        entry_id,
        spans: vec![Span::styled(
            format!("⋮ 已折叠{position} {omitted} 行"),
            theme::faint(),
        )],
        links: Vec::new(),
        action: RowAction::Fold(section),
        copy_meta: RowCopyMeta {
            decorative: true,
            ..Default::default()
        },
    }
}
