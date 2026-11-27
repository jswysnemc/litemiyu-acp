//! 输入框上方补全悬浮菜单与权限审批抽屉渲染模块。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::acp::PermissionRequest;
use crate::completion::{CompletionKind, CompletionMenu};
use crate::linebreak;
use crate::theme;

use super::HoverTarget;
use super::overlay_hits::{
    CompletionHits, OverlayFooterAction, PermissionHits, build_right_hint_line,
};

/// 绘制输入框上方的统一悬浮补全菜单（支持 `/` 命令、`#` Skills 与 `@` 工作区文件），并返回鼠标命中热区。
///
/// 参数:
/// - `frame`: 当前渲染帧
/// - `input_frame`: 输入框外框区域
/// - `menu`: 补全菜单快照
/// - `selected`: 当前选中项索引
/// - `hover`: 当前悬停目标（用于顶部提示按钮前景高亮）
///
/// 返回:
/// - 补全菜单外框及各可见候选项的鼠标热区
pub fn draw_completion_menu(
    frame: &mut Frame,
    input_frame: Rect,
    menu: &CompletionMenu,
    selected: usize,
    hover: &HoverTarget,
) -> CompletionHits {
    let suggestions = &menu.items;
    if suggestions.is_empty() || input_frame.y < 3 {
        return CompletionHits::default();
    }
    let visible = suggestions.len().min(8);
    let height = visible as u16 + 2;
    let y = input_frame.y.saturating_sub(height);
    let area = Rect::new(input_frame.x, y, input_frame.width, height);
    let mut hits = CompletionHits {
        area,
        items: Vec::with_capacity(visible),
        footer_actions: Vec::new(),
    };

    let (title, accept_label) = match menu.kind {
        CompletionKind::Command => (format!(" 命令 ({}) ", suggestions.len()), "Enter 执行"),
        CompletionKind::Skill => (format!(" Skills ({}) ", suggestions.len()), "Enter 插入"),
        CompletionKind::File => (format!(" 工作区文件 ({}) ", suggestions.len()), "Enter 插入"),
    };
    let (hint_line, footer_actions) = build_right_hint_line(
        area,
        area.y,
        &[
            ("↑↓/点击 选择 · ", None),
            (accept_label, Some(OverlayFooterAction::AcceptCompletion)),
            (" · ", None),
            ("Esc 关闭", Some(OverlayFooterAction::Close)),
        ],
        hover,
    );
    hits.footer_actions = footer_actions;

    let p = theme::palette();
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::faint())
        .title_top(Line::styled(title, theme::accent()).left_aligned())
        .title_top(hint_line)
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let start = if selected >= visible {
        selected + 1 - visible
    } else {
        0
    };
    let mut lines = Vec::new();
    let max_w = usize::from(inner.width);
    let label_cap = match menu.kind {
        CompletionKind::File => 28,
        CompletionKind::Command | CompletionKind::Skill => 24,
    };
    for (row_offset, (idx, item)) in suggestions
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        if row_offset < usize::from(inner.height) {
            hits.items.push((
                Rect::new(inner.x, inner.y + row_offset as u16, inner.width, 1),
                idx,
            ));
        }
        let is_sel = idx == selected;
        let clipped_label = linebreak::clip(&item.label, label_cap);
        let label_w = clipped_label.width().max(16);
        let pad_after_label = label_w.saturating_sub(clipped_label.width());
        let desc_room = max_w.saturating_sub(label_w + 4);
        let desc = linebreak::clip(&item.description, desc_room as u16);
        let row_text = format!(
            " {}{}  {}",
            clipped_label,
            " ".repeat(pad_after_label),
            desc
        );

        if is_sel {
            let pad = max_w.saturating_sub(row_text.width());
            lines.push(Line::styled(
                format!("{row_text}{}", " ".repeat(pad)),
                theme::picked_bar(),
            ));
        } else {
            let label_style = match menu.kind {
                CompletionKind::File => theme::md_url(),
                CompletionKind::Skill => theme::picked(),
                CompletionKind::Command => {
                    if item.accent_primary {
                        theme::accent()
                    } else {
                        theme::picked()
                    }
                }
            };
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(clipped_label, label_style),
                Span::raw(" ".repeat(pad_after_label + 2)),
                Span::styled(desc, theme::dim()),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
    hits
}

/// 绘制底部权限审批抽屉，并返回各权限选项行的鼠标命中热区。
///
/// 参数:
/// - `frame`: 当前渲染帧
/// - `area`: 抽屉外框区域
/// - `req`: 服务端发起的权限请求
/// - `selected`: 当前高亮选项索引
///
/// 返回:
/// - 抽屉外框与各可见选项行的鼠标热区
pub fn draw_permission_drawer(
    frame: &mut Frame,
    area: Rect,
    req: &PermissionRequest,
    selected: usize,
    hover: &HoverTarget,
) -> PermissionHits {
    let mut hits = PermissionHits {
        area,
        options: Vec::with_capacity(req.options.len()),
        footer_actions: Vec::new(),
    };
    let p = theme::palette();
    frame.render_widget(Clear, area);
    let bottom_y = area.bottom().saturating_sub(1);
    let (hint_line, footer_actions) = build_right_hint_line(
        area,
        bottom_y,
        &[
            ("↑↓/点击 选择 · ", None),
            ("Enter 确认", Some(OverlayFooterAction::Confirm)),
            (" · ", None),
            ("Esc 拒绝", Some(OverlayFooterAction::Close)),
        ],
        hover,
    );
    hits.footer_actions = footer_actions;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::warn())
        .title_top(Line::styled(" ◈ 权限确认 ", theme::warn()))
        .title_bottom(hint_line)
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("工具: ", theme::dim()),
        Span::styled(
            linebreak::clip(&req.title, inner.width.saturating_sub(6)),
            theme::accent_bold(),
        ),
    ]));

    if let Some(raw) = &req.raw_input {
        if let Some(cmd) = raw["CommandLine"]
            .as_str()
            .or_else(|| raw["command"].as_str())
        {
            let one_line = cmd.replace('\n', " ");
            let clipped = linebreak::clip(&one_line, inner.width.saturating_sub(2));
            let mut spans = vec![Span::styled("$ ", theme::picked())];
            for piece in crate::markdown::code::highlight_shell_line(&clipped, None) {
                spans.push(Span::styled(piece.text, piece.style));
            }
            lines.push(Line::from(spans));
        } else {
            let preview = raw["Path"]
                .as_str()
                .or_else(|| raw["path"].as_str())
                .map(String::from)
                .unwrap_or_else(|| raw.to_string());
            lines.push(Line::styled(
                linebreak::clip(&preview.replace('\n', " "), inner.width),
                theme::md_code(),
            ));
        }
    }
    lines.push(Line::default());

    let max_w = usize::from(inner.width);
    for (idx, opt) in req.options.iter().enumerate() {
        let row_offset = lines.len();
        if row_offset < usize::from(inner.height) {
            hits.options.push((
                Rect::new(inner.x, inner.y + row_offset as u16, inner.width, 1),
                idx,
            ));
        }
        let is_sel = idx == selected;
        let label = format!(" [{}] {}", idx + 1, opt.name);
        if is_sel {
            let pad = max_w.saturating_sub(label.width());
            lines.push(Line::styled(
                format!("{label}{}", " ".repeat(pad)),
                theme::picked_bar(),
            ));
        } else {
            let style = if opt.kind.contains("reject") {
                theme::error()
            } else if opt.kind.contains("allow") {
                theme::good()
            } else {
                theme::text()
            };
            lines.push(Line::styled(label, style));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
    hits
}
