//! 正文折叠提示、自适应展开按钮与底部收起操作条构建模块。

use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use super::{RenderRow, RowAction, RowCopyMeta};
use crate::{theme, transcript::FoldSection};

/// 正文行内操作按钮的语义动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineAction {
    /// 增量展开指定步长（仅在剩余行数大于步长时展示）
    RevealStep { section: FoldSection, step: usize },
    /// 一次性展开该分区全部剩余行
    RevealAll { section: FoldSection },
    /// 将局部读取工具切换为查看完整本地文件
    ReadFullFile,
    /// 收起当前展开条目（等效于点击顶部标题行）
    CollapseEntry,
}

/// 单个行内操作按钮在当前行内的列范围（相对正文左边界 `body.x`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineButton {
    pub col_start: u16,
    pub col_end: u16,
    pub action: InlineAction,
}

/// 构造折叠/底部操作条时的可选配置。
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ActionBarOptions {
    /// 需要呈现折叠信息的分区与实际被隐藏的视觉行数
    pub fold: Option<(FoldSection, usize)>,
    /// 是否展示「查看完整文件」按钮（仅局部读取且本地文件存在更多行时为真）
    pub show_full_file: bool,
    /// 是否在尾部附带「▴ 收起」按钮
    pub show_collapse: bool,
}

/// 【正文】【智能操作条】根据实际剩余行数与分区步长构建紧凑的行内按钮条（无背景色，悬停仅前景高亮）。
///
/// 参数:
/// - `entry_id`: 时间线条目 ID
/// - `opts`: 折叠分区、完整文件与收起按钮配置
///
/// 返回:
/// - 若存在任意需渲染的操作项则返回 `Some(RenderRow)`，否则返回 `None`
pub(super) fn action_bar(entry_id: u64, opts: ActionBarOptions) -> Option<RenderRow> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buttons: Vec<InlineButton> = Vec::new();
    let mut col: usize = 0;

    let push_plain = |spans: &mut Vec<Span<'static>>, col: &mut usize, text: String| {
        *col += text.width();
        spans.push(Span::styled(text, theme::faint()));
    };
    let push_btn = |spans: &mut Vec<Span<'static>>,
                    buttons: &mut Vec<InlineButton>,
                    col: &mut usize,
                    label: String,
                    action: InlineAction| {
        let start = u16::try_from(*col).unwrap_or(u16::MAX);
        let w = label.width();
        *col += w;
        let end = u16::try_from(*col).unwrap_or(u16::MAX);
        spans.push(Span::styled(label, theme::dim()));
        buttons.push(InlineButton {
            col_start: start,
            col_end: end,
            action,
        });
    };

    // 1. 若存在未展开的剩余视觉行，按「剩余行数 vs 分区步长」生成贴合实际的按钮文案
    let mut fold_section = None;
    if let Some((section, omitted)) = opts.fold
        && omitted > 0
    {
        fold_section = Some(section);
        let step = section.reveal_step();
        let prefix = if section == FoldSection::Thought {
            format!("⋮ 前面还有 {omitted} 行")
        } else {
            format!("⋮ 剩余 {omitted} 行")
        };
        push_plain(&mut spans, &mut col, prefix);
        push_plain(&mut spans, &mut col, " · ".to_string());

        if omitted > step {
            push_btn(
                &mut spans,
                &mut buttons,
                &mut col,
                format!("+{step} 行"),
                InlineAction::RevealStep { section, step },
            );
            push_plain(&mut spans, &mut col, " · ".to_string());
            push_btn(
                &mut spans,
                &mut buttons,
                &mut col,
                "展开全部".to_string(),
                InlineAction::RevealAll { section },
            );
        } else {
            // 剩余行数不超过步长时，不展示冗余的 `+{step} 行`，直接给出精准的一次性展开文案
            let exact_label = if section == FoldSection::Thought {
                format!("展开前 {omitted} 行")
            } else {
                format!("展开剩余 {omitted} 行")
            };
            push_btn(
                &mut spans,
                &mut buttons,
                &mut col,
                exact_label,
                InlineAction::RevealAll { section },
            );
        }
    }

    // 2. 局部读取可查看完整文件时追加直达按钮
    if opts.show_full_file {
        if !spans.is_empty() {
            push_plain(&mut spans, &mut col, " · ".to_string());
        } else {
            push_plain(&mut spans, &mut col, "  ".to_string());
        }
        push_btn(
            &mut spans,
            &mut buttons,
            &mut col,
            "查看完整文件".to_string(),
            InlineAction::ReadFullFile,
        );
    }

    // 3. 底部统一收起入口
    if opts.show_collapse {
        if !spans.is_empty() {
            push_plain(&mut spans, &mut col, " · ".to_string());
        } else {
            push_plain(&mut spans, &mut col, "  ".to_string());
        }
        push_btn(
            &mut spans,
            &mut buttons,
            &mut col,
            "▴ 收起".to_string(),
            InlineAction::CollapseEntry,
        );
    }

    if buttons.is_empty() {
        return None;
    }

    Some(RenderRow {
        entry_id,
        spans,
        links: Vec::new(),
        buttons,
        action: fold_section.map_or(RowAction::None, RowAction::Fold),
        copy_meta: RowCopyMeta {
            decorative: true,
            ..Default::default()
        },
    })
}

/// 生成仅包含底部「▴ 收起」按钮的精简操作行。
///
/// 参数:
/// - `entry_id`: 时间线条目 ID
///
/// 返回:
/// - 包含单个 `CollapseEntry` 按钮的 `RenderRow`
pub(super) fn collapse_bar(entry_id: u64) -> RenderRow {
    action_bar(
        entry_id,
        ActionBarOptions {
            fold: None,
            show_full_file: false,
            show_collapse: true,
        },
    )
    .expect("collapse bar always contains a button")
}
