//! 浮层选择器、权限审批抽屉、代理提问表单与输入补全菜单的鼠标命中热区及边框动作按钮构造。

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::theme;

use super::HoverTarget;
use super::overlay::ResumeScope;

/// 弹窗/抽屉底栏或顶栏提示条中的可交互动作按钮标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayFooterAction {
    /// 确认/提交当前高亮项（等效 `Enter`）
    Confirm,
    /// 关闭弹窗 / 拒绝权限 / 取消提问 / 收起补全（等效 `Esc`）
    Close,
    /// `/resume` 切换当前目录与全部会话范围（等效 `Tab`）
    ToggleResumeScope,
    /// URL 模式代理提问直接打开链接（等效 `O`）
    OpenUrl,
    /// 补全菜单接受当前高亮项（等效 `Enter`）
    AcceptCompletion,
}

/// 模态选择器（模型/思考等级/模式/状态/恢复会话/主题/队列/帮助）鼠标热区。
#[derive(Default, Debug, Clone)]
pub struct PickerHits {
    /// 弹窗外框整体区域（点击外框之外空白处关闭浮层）
    pub popup: Rect,
    /// 可点击的列表项热区：`(行或卡片区域, filtered() 列表索引)`
    pub items: Vec<(Rect, usize)>,
    /// `/resume` 顶部范围标签页热区：`(标签区域, 目标目录范围)`
    pub resume_tabs: Vec<(Rect, ResumeScope)>,
    /// 底部边框提示动作按钮热区（如 `Enter 确认`、`Esc/点外部 关闭`）
    pub footer_actions: Vec<(Rect, OverlayFooterAction)>,
}

/// 权限审批抽屉鼠标热区。
#[derive(Default, Debug, Clone)]
pub struct PermissionHits {
    /// 审批抽屉外框区域
    pub area: Rect,
    /// 可点击选项行热区：`(选项行区域, options 索引)`
    pub options: Vec<(Rect, usize)>,
    /// 底部边框提示动作按钮热区（如 `Enter 确认`、`Esc 拒绝`）
    pub footer_actions: Vec<(Rect, OverlayFooterAction)>,
}

/// `elicitation/create` 交互式提问抽屉鼠标热区。
#[derive(Default, Debug, Clone)]
pub struct ElicitationHits {
    /// 提问抽屉外框区域
    pub area: Rect,
    /// 多题页签热区：`(页签区域, field 索引)`
    pub tabs: Vec<(Rect, usize)>,
    /// 选项与自定义输入行热区：`(行区域, cursor_opt 索引, 是否为自定义输入行)`
    pub options: Vec<(Rect, usize, bool)>,
    /// URL 确认模式下的可点击链接区域
    pub url_row: Option<Rect>,
    /// 底部边框提示动作按钮热区（如 `Enter 提交`、`O 打开链接`、`Esc 取消`）
    pub footer_actions: Vec<(Rect, OverlayFooterAction)>,
}

/// 输入框上方悬浮补全菜单（`/` 命令、`#` Skills、`@` 文件）鼠标热区。
#[derive(Default, Debug, Clone)]
pub struct CompletionHits {
    /// 补全菜单外框区域
    pub area: Rect,
    /// 补全候选项行热区：`(行区域, menu.items 索引)`
    pub items: Vec<(Rect, usize)>,
    /// 顶部边框提示动作按钮热区（如 `Enter 执行`、`Esc 关闭`）
    pub footer_actions: Vec<(Rect, OverlayFooterAction)>,
}

/// 单帧内所有活跃浮层与抽屉的鼠标命中热区汇总。
#[derive(Default, Debug, Clone)]
pub struct OverlayHits {
    pub picker: PickerHits,
    pub permission: PermissionHits,
    pub elicitation: ElicitationHits,
    pub completion: CompletionHits,
}

/// 【浮层】【边框提示按钮】构造右对齐边框提示行，输出每个可点击动作按钮的精确热区；悬停仅高亮前景无背景。
///
/// 参数:
/// - `block_area`: 外框 `Block` 的完整矩形（含左右边框）
/// - `y`: 该边框提示所在的屏幕行（顶边框 `area.y` 或底边框 `area.bottom() - 1`）
/// - `segments`: 提示片段列表，`(显示文案, 关联动作)`；`None` 表示普通说明或分隔符
/// - `hover`: 当前全局悬停目标
///
/// 返回:
/// - `(右对齐边框 Line, 可点击按钮热区集合)`
pub fn build_right_hint_line(
    block_area: Rect,
    y: u16,
    segments: &[(&str, Option<OverlayFooterAction>)],
    hover: &HoverTarget,
) -> (Line<'static>, Vec<(Rect, OverlayFooterAction)>) {
    let mut spans = Vec::with_capacity(segments.len() + 2);
    let mut hits = Vec::new();
    let content_w: usize = segments.iter().map(|(t, _)| t.width()).sum();
    let total_w = u16::try_from(content_w + 2).unwrap_or(u16::MAX);

    let inner_left = block_area.x.saturating_add(1);
    let inner_right = block_area.right().saturating_sub(1);
    let inner_w = inner_right.saturating_sub(inner_left);
    let start_x = if total_w >= inner_w {
        inner_left
    } else {
        inner_right.saturating_sub(total_w)
    };

    spans.push(Span::styled(" ", theme::faint()));
    let mut cursor_x = start_x.saturating_add(1);
    for &(text, action) in segments {
        let seg_w = u16::try_from(text.width()).unwrap_or(0);
        if let Some(act) = action {
            let hovered = matches!(hover, HoverTarget::OverlayAction(a) if *a == act);
            // 悬停时仅高亮前景文字，不铺设任何背景色
            let style = if hovered {
                theme::accent_bold()
            } else {
                theme::dim()
            };
            if cursor_x < inner_right && seg_w > 0 {
                let clipped_w = seg_w.min(inner_right.saturating_sub(cursor_x));
                hits.push((Rect::new(cursor_x, y, clipped_w, 1), act));
            }
            spans.push(Span::styled(text.to_string(), style));
        } else {
            spans.push(Span::styled(text.to_string(), theme::faint()));
        }
        cursor_x = cursor_x.saturating_add(seg_w);
    }
    spans.push(Span::styled(" ", theme::faint()));

    (Line::from(spans).right_aligned(), hits)
}
