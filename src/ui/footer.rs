//! 底部状态栏渲染模块：左侧展示模型/思考等级/模式/队列与后台计数，
//! 右侧对上下文用量、累计 Token 与缓存命中、当前工作目录及 Git 分支进行分层语义着色。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::QueuedMode;
use crate::linebreak;
use crate::paths;
use crate::theme;
use crate::transcript::format_short;

use super::{HoverTarget, ViewState, overlay};

/// Git 分支名在底栏中的最大显示列宽（超出时尾部省略，避免挤占路径与用量）。
const MAX_BRANCH_DISPLAY_WIDTH: usize = 20;

/// 绘制底部单行状态栏并返回右侧上下文/用量区点击热区。
///
/// 参数:
/// - `frame`: 当前渲染帧
/// - `area`: 底部状态栏矩形
/// - `view`: 视图只读状态快照
/// - `show_cwd`: 是否允许在右侧展示工作目录与 Git 分支
///
/// 返回:
/// - 上下文/用量区域点击热区（未渲染时返回 `Rect::default()`）
pub(super) fn draw(frame: &mut Frame, area: Rect, view: &ViewState<'_>, show_cwd: bool) -> Rect {
    // 1. 绘制左侧模型、思考等级、模式、排队及后台任务状态
    let left_spans = build_left_spans(view);
    let left_w: usize = left_spans.iter().map(|s| s.content.width()).sum();
    frame.render_widget(Paragraph::new(Line::from(left_spans)), area);

    let avail = usize::from(area.width).saturating_sub(left_w + 2);
    if avail == 0 {
        return Rect::default();
    }

    // 2. 构建右侧上下文用量与累计 Token/缓存命中着色片段
    let hover_usage = matches!(view.hover, HoverTarget::UsageCell);
    let (usage_spans, usage_w) = build_usage_spans(view, hover_usage);
    let show_usage = usage_w > 0 && usage_w <= avail;

    // 3. 计算工作目录与 Git 分支的可用列宽预算
    let branch_label = view
        .git_branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|b| linebreak::clip(b, MAX_BRANCH_DISPLAY_WIDTH as u16));
    let branch_w = branch_label
        .as_ref()
        .map_or(0, |b| b.width() + 3); // " (" + branch + ")"

    let sep_w = if show_usage { 3 } else { 0 };
    let tail_avail = if show_usage {
        avail.saturating_sub(usage_w + sep_w)
    } else {
        avail
    };

    let collapsed_cwd = if show_cwd && !view.cwd.trim().is_empty() {
        paths::collapse_home_dir(view.cwd.trim())
    } else {
        String::new()
    };

    let (cwd_text, show_branch) = if !collapsed_cwd.is_empty() && tail_avail >= 6 {
        let include_branch = branch_w > 0 && tail_avail >= branch_w + 8;
        let cwd_cap = if include_branch {
            (tail_avail - branch_w).min(36)
        } else {
            tail_avail.min(44)
        };
        let short = overlay::shorten_path_left(&collapsed_cwd, cwd_cap as u16);
        (short, include_branch)
    } else if branch_w > 0 && show_cwd && tail_avail >= branch_w {
        (String::new(), true)
    } else {
        (String::new(), false)
    };

    // 4. 组合右侧全部着色片段并记录目录+分支总列宽，精确对齐 usage_cell 点击热区
    let mut right_spans: Vec<Span<'static>> = Vec::new();
    let mut tail_w_used = 0usize;

    if show_usage {
        right_spans.extend(usage_spans);
    }

    let has_tail = !cwd_text.is_empty() || show_branch;
    if has_tail {
        if show_usage {
            right_spans.push(Span::styled(" · ", theme::faint()));
            tail_w_used += 3;
        }
        if !cwd_text.is_empty() {
            tail_w_used += cwd_text.width();
            right_spans.push(Span::styled(cwd_text.clone(), theme::md_url()));
        }
        if show_branch
            && let Some(branch) = branch_label
        {
            let p = theme::palette();
            let prefix = if cwd_text.is_empty() { "(" } else { " (" };
            tail_w_used += prefix.width() + branch.width() + 1;
            right_spans.push(Span::styled(prefix, theme::faint()));
            right_spans.push(Span::styled(branch, Style::new().fg(p.picked)));
            right_spans.push(Span::styled(")", theme::faint()));
        }
    }

    if !right_spans.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(right_spans).right_aligned()),
            area,
        );
    }

    if show_usage {
        let w = u16::try_from(usage_w).unwrap_or(0).min(area.width);
        let right_offset = u16::try_from(tail_w_used).unwrap_or(0);
        let x = area
            .right()
            .saturating_sub(right_offset)
            .saturating_sub(w)
            .max(area.x);
        return Rect::new(x, area.y, w, 1);
    }
    Rect::default()
}

/// 组装底栏左侧模型与运行状态片段。
fn build_left_spans(view: &ViewState<'_>) -> Vec<Span<'static>> {
    let mut left_spans = vec![Span::styled(view.model_label.to_string(), theme::model())];
    if !view.effort_label.is_empty() {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(view.effort_label.to_string(), theme::accent()));
    }
    if !view.mode_label.is_empty() {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(view.mode_label.to_string(), theme::dim()));
    }
    if view.queued_mode == QueuedMode::SendImmediately {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(view.queued_mode.short_label(), theme::accent()));
    }
    if view.queued_count > 0 {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(
            format!("队列 {}", view.queued_count),
            theme::accent(),
        ));
    }
    if !view.background_tasks.is_empty() {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(
            format!("后台 {}", view.background_tasks.len()),
            theme::accent(),
        ));
    }
    left_spans
}

/// 组装右侧上下文占用与累计 Token/缓存命中语义着色片段，并返回总显示宽度。
fn build_usage_spans(view: &ViewState<'_>, hovered: bool) -> (Vec<Span<'static>>, usize) {
    let p = theme::palette();
    let style_with_hover = |base: Style| {
        if hovered {
            base.bg(p.row_focus_bg).add_modifier(Modifier::UNDERLINED)
        } else {
            base
        }
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut width = 0usize;

    // 1. 上下文窗口用量（按占用比例自动呈现安全青 / 预警黄 / 高载红）
    if view.transcript.context_window > 0 && view.transcript.context_used > 0 {
        let pct =
            (view.transcript.context_used as f64 / view.transcript.context_window as f64) * 100.0;
        let ctx_style = if pct >= 85.0 {
            theme::error()
        } else if pct >= 60.0 {
            theme::warn()
        } else {
            theme::thought()
        };
        let ctx_text = format!(
            "{}/{}({:.1}%)",
            format_short(view.transcript.context_used),
            format_short(view.transcript.context_window),
            pct
        );
        width += ctx_text.width();
        spans.push(Span::styled(ctx_text, style_with_hover(ctx_style)));
    }

    // 2. 累计 Token 总量（主题强调色）与缓存命中率（高命中柔和绿）
    let total = view.transcript.total_input
        + view.transcript.total_output
        + view.transcript.total_cached_read;
    if total > 0 {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", style_with_hover(theme::faint())));
            width += 3;
        }
        let sum_text = format!("Σ{}", format_short(total));
        width += sum_text.width();
        spans.push(Span::styled(sum_text, style_with_hover(theme::accent())));

        let denom = view.transcript.total_input + view.transcript.total_cached_read;
        if denom > 0 && view.transcript.total_cached_read > 0 {
            let hit_pct = (view.transcript.total_cached_read * 100) / denom;
            let hit_style = if hit_pct >= 50 {
                theme::good()
            } else {
                theme::dim()
            };
            let hit_text = format!("(C{hit_pct}%)");
            width += hit_text.width();
            spans.push(Span::styled(hit_text, style_with_hover(hit_style)));
        }
    }

    (spans, width)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    use crate::app::App;
    use crate::theme::{self, ThemePreset};

    #[test]
    fn footer_colors_usage_cache_cwd_and_branch_distinctly_and_aligns_usage_hitbox() {
        theme::init(ThemePreset::CatppuccinMocha, true);
        let p = theme::palette();

        let dir = std::env::temp_dir().join(format!("agy_tui_footer_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let mut app = App::new(None, dir.to_string_lossy().into_owned(), None, false, true);
        app.transcript.context_used = 210_200;
        app.transcript.context_window = 1_000_000;
        app.transcript.total_input = 240_000;
        app.transcript.total_output = 60_000;
        app.transcript.total_cached_read = 3_700_000;

        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| app.render(f)).unwrap();

        let buf = term.backend().buffer();
        let footer_y = 29u16;
        let mut row_text = String::new();
        for x in 0..120u16 {
            let sym = buf[(x, footer_y)].symbol();
            row_text.push_str(sym);
        }

        // 1. 验证右侧包含上下文、累计用量、缓存命中率、工作目录与 Git 分支
        assert!(row_text.contains("210.2k/1M(21.0%)"), "missing context usage: {row_text:?}");
        assert!(row_text.contains("Σ4M"), "missing total tokens: {row_text:?}");
        assert!(row_text.contains("(C93%)"), "missing cache hit rate: {row_text:?}");
        assert!(row_text.contains("(main)"), "missing git branch: {row_text:?}");

        // 2. 验证四段采用不同的语义前景色
        let find_col = |needle: &str| -> u16 {
            let byte_idx = row_text.find(needle).expect("needle in footer");
            row_text[..byte_idx].width() as u16
        };
        let ctx_col = find_col("210.2k");
        let sum_col = find_col("Σ4M");
        let cache_col = find_col("(C93%)");
        let branch_col = find_col("main");

        assert_eq!(buf[(ctx_col, footer_y)].fg, p.thought);
        assert_eq!(buf[(sum_col, footer_y)].fg, p.accent);
        assert_eq!(buf[(cache_col, footer_y)].fg, p.good);
        assert_eq!(buf[(branch_col, footer_y)].fg, p.picked);

        // 3. 验证 usage_cell 热区严格覆盖用量段，不偏移到路径/分支上
        let hit = app.last_hits.usage_cell;
        assert_eq!(hit.y, footer_y);
        assert_eq!(hit.x, ctx_col);
        assert!(hit.right() <= find_col("(main)"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
