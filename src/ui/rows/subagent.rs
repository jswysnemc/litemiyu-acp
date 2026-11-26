//! 子代理正文卡片：窄屏也能查看摘要，不展示实时步骤。

use ratatui::text::Span;

use super::{RenderRow, RowAction, RowCopyMeta};
use crate::subagent::{Agent, Phase};
use crate::{linebreak, theme};

/// 【子代理】【绘制摘要】为 `entry_id` 的子任务生成卡片；`expanded` 控制详情，`width` 限定列宽。
pub(super) fn append(
    entry_id: u64,
    agents: &[Agent],
    expanded: bool,
    width: u16,
    rows: &mut Vec<RenderRow>,
) {
    for agent in agents.iter().filter(|agent| agent.entry_id == entry_id) {
        let mark = if expanded { "▾" } else { "▸" };
        let title = format!("{mark} 子代理 · {}", agent.headline());
        let style = match agent.phase {
            Phase::Failed => theme::error(),
            Phase::Completed => theme::good(),
            _ => theme::accent(),
        };
        rows.push(RenderRow {
            entry_id,
            spans: vec![Span::styled(linebreak::clip(&title, width), style)],
            links: Vec::new(),
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta::default(),
        });
        if expanded {
            for detail in agent.details() {
                rows.push(RenderRow {
                    entry_id,
                    spans: vec![Span::styled(
                        linebreak::clip(&format!("  {detail}"), width),
                        theme::dim(),
                    )],
                    links: Vec::new(),
                    action: RowAction::Toggle,
                    copy_meta: RowCopyMeta::default(),
                });
            }
        }
    }
}
