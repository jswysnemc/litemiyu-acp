//! 主题与配色引擎（参考 LiteMiyu `theme/` + Material Design 3 `SchemeContent` 衍生调色板）。
//!
//! 统一管理 14 套内置精选主题与 `$XDG_CONFIG_HOME/agy-tui/themes/*.json` 用户自定义主题，
//! 通过单调递增的 `revision()` 通知 Markdown 缓存自动失效。

mod custom;
mod palette;
mod presets;

use std::path::PathBuf;
use std::sync::{LazyLock, PoisonError, RwLock};

use ratatui::style::{Color, Modifier, Style};

pub use custom::{CustomTheme, export_theme_template, load_custom_themes_from_dir};
pub use palette::Palette;
pub use presets::ThemePreset;

use palette::{apply_transparent_mode, swatch_colors};
use presets::NIGHT;

/// 供 `/theme` 选择器与界面展示的统一主题描述项。
#[derive(Debug, Clone)]
pub struct ThemeDescriptor {
    pub id: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
enum ActiveSource {
    Builtin(ThemePreset),
    Custom(String),
}

struct ThemeState {
    source: ActiveSource,
    paint_bg: bool,
    palette: Palette,
    custom_themes: Vec<CustomTheme>,
    revision: u64,
}

static THEME: LazyLock<RwLock<ThemeState>> = LazyLock::new(|| {
    RwLock::new(ThemeState {
        source: ActiveSource::Builtin(ThemePreset::MiyuLavender),
        paint_bg: true,
        palette: NIGHT,
        custom_themes: Vec::new(),
        revision: 1,
    })
});

/// 按内置预设与铺底标志初始化全局主题（供单元测试快速设定主题状态）。
///
/// 参数:
/// - `preset`: 内置主题预设
/// - `paint_bg`: 是否绘制实体背景色
#[cfg(test)]
pub fn init(preset: ThemePreset, paint_bg: bool) {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    let effective_bg = paint_bg && preset != ThemePreset::Terminal;
    state.source = ActiveSource::Builtin(preset);
    state.paint_bg = effective_bg;
    state.palette = build_preset_palette(preset, effective_bg);
    state.revision = state.revision.wrapping_add(1);
}

/// 加载用户自定义主题目录并按主题 ID（支持内置 ID/别名与自定义 ID）初始化主题。
///
/// 参数:
/// - `theme_id`: 目标主题 ID 或别名
/// - `paint_bg`: 用户偏好的铺底开关
///
/// 返回:
/// - 命中对应主题返回 `true`，未命中并回退默认主题返回 `false`
pub fn init_by_id(theme_id: &str, paint_bg: bool) -> bool {
    let custom = crate::paths::themes_dir()
        .map(|dir| load_custom_themes_from_dir(&dir))
        .unwrap_or_default();
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    state.custom_themes = custom;
    if apply_source_by_id(&mut state, theme_id, Some(paint_bg)).is_some() {
        return true;
    }
    state.source = ActiveSource::Builtin(ThemePreset::MiyuLavender);
    state.paint_bg = paint_bg;
    state.palette = build_preset_palette(ThemePreset::MiyuLavender, paint_bg);
    state.revision = state.revision.wrapping_add(1);
    false
}

/// 重新扫描用户自定义主题目录；若当前正使用自定义主题则立即热更新其调色板。
///
/// 返回:
/// - 当前已加载的自定义主题总数
pub fn reload_custom_themes() -> usize {
    let custom = crate::paths::themes_dir()
        .map(|dir| load_custom_themes_from_dir(&dir))
        .unwrap_or_default();
    let count = custom.len();
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    state.custom_themes = custom;
    rebuild_active_palette(&mut state);
    count
}

/// 按主题 ID 或别名切换主题（支持内置与自定义主题）。
///
/// 参数:
/// - `id`: 内置主题 ID/别名或自定义主题 ID
///
/// 返回:
/// - 切换成功返回新主题的展示标签，未找到返回 `None`
pub fn set_by_id(id: &str) -> Option<String> {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    apply_source_by_id(&mut state, id, None)
}

/// 预览指定主题而不持久化（供 `/theme` 选择器上下切换即时预览）。
///
/// 参数:
/// - `id`: 目标主题 ID
/// - `base_paint_bg`: 进入选择器时的原始铺底开关
///
/// 返回:
/// - 是否成功切换预览
pub fn preview_by_id(id: &str, base_paint_bg: bool) -> bool {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    apply_source_by_id(&mut state, id, Some(base_paint_bg)).is_some()
}

/// 取消预览时精确还原原主题与铺底开关。
///
/// 参数:
/// - `id`: 原始主题 ID
/// - `paint_bg`: 原始铺底开关
pub fn restore_state(id: &str, paint_bg: bool) {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    if apply_source_by_id(&mut state, id, Some(paint_bg)).is_none() {
        state.source = ActiveSource::Builtin(ThemePreset::MiyuLavender);
        state.paint_bg = paint_bg;
        rebuild_active_palette(&mut state);
    }
}

/// 切换当前主题的铺底色 / 透明背景状态。
///
/// 返回:
/// - 切换后的 `paint_bg` 状态（`true` 表示铺底色，`false` 表示透明继承终端）
pub fn toggle_bg() -> bool {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    state.paint_bg = !state.paint_bg;
    rebuild_active_palette(&mut state);
    state.paint_bg
}

/// 将当前激活主题导出为可编辑 JSON 模板文件。
///
/// 参数:
/// - `raw_name`: 导出文件名（不含扩展名），为空时使用 `"custom"`
///
/// 返回:
/// - 成功返回写入路径并自动重载自定义主题列表，失败返回错误信息
pub fn export_current_theme(raw_name: &str) -> Result<PathBuf, String> {
    let dir = crate::paths::themes_dir().ok_or_else(|| "无法解析用户配置目录".to_string())?;
    let (base_id, base_palette, transparent) = {
        let state = THEME.read().unwrap_or_else(PoisonError::into_inner);
        let (base_id, base_palette, transparent) = match &state.source {
            ActiveSource::Builtin(p) => (
                p.id().to_string(),
                p.base_palette(),
                !state.paint_bg || *p == ThemePreset::Terminal,
            ),
            ActiveSource::Custom(cid) => {
                if let Some(ct) = state.custom_themes.iter().find(|t| t.id == *cid) {
                    (
                        ct.base_preset.id().to_string(),
                        ct.palette,
                        !state.paint_bg || ct.transparent,
                    )
                } else {
                    (
                        ThemePreset::MiyuLavender.id().to_string(),
                        state.palette,
                        !state.paint_bg,
                    )
                }
            }
        };
        (base_id, base_palette, transparent)
    };
    let saved = export_theme_template(&dir, raw_name, &base_id, &base_palette, transparent)?;
    let _ = reload_custom_themes();
    Ok(saved)
}

/// 返回全部可选主题列表（全部内置预设 + 已加载的用户自定义主题）。
pub fn list_themes() -> Vec<ThemeDescriptor> {
    let state = THEME.read().unwrap_or_else(PoisonError::into_inner);
    let mut list = Vec::with_capacity(ThemePreset::ALL.len() + state.custom_themes.len());
    for &preset in ThemePreset::ALL {
        list.push(ThemeDescriptor {
            id: preset.id().to_string(),
            label: preset.label().to_string(),
            detail: format!("{} · {}", preset.category_tag(), preset.id()),
        });
    }
    for custom in &state.custom_themes {
        let mode_tag = if custom.transparent {
            "自定义·透明"
        } else {
            "自定义"
        };
        list.push(ThemeDescriptor {
            id: custom.id.clone(),
            label: custom.label.clone(),
            detail: format!("{mode_tag} · {}", custom.id),
        });
    }
    list
}

/// 按主题 ID 查询用于选择器展示的 5 色代表色板。
///
/// 参数:
/// - `id`: 内置或自定义主题 ID
///
/// 返回:
/// - 命中时返回 `[Color; 5]`，否则返回 `None`
pub fn swatches_for_id(id: &str) -> Option<[Color; 5]> {
    if let Some(preset) = ThemePreset::parse(id) {
        return Some(swatch_colors(&preset.base_palette()));
    }
    let state = THEME.read().unwrap_or_else(PoisonError::into_inner);
    state
        .custom_themes
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(id))
        .map(|t| swatch_colors(&t.palette))
}

/// 返回当前激活主题对应的内置预设（自定义主题时返回其基底预设）。
#[allow(dead_code)]
pub fn current_preset() -> ThemePreset {
    let state = THEME.read().unwrap_or_else(PoisonError::into_inner);
    match &state.source {
        ActiveSource::Builtin(p) => *p,
        ActiveSource::Custom(cid) => state
            .custom_themes
            .iter()
            .find(|t| t.id == *cid)
            .map(|t| t.base_preset)
            .unwrap_or(ThemePreset::MiyuLavender),
    }
}

/// 返回当前激活主题的稳定 ID（用于持久化与首页展示）。
pub fn current_theme_id() -> String {
    let state = THEME.read().unwrap_or_else(PoisonError::into_inner);
    match &state.source {
        ActiveSource::Builtin(p) => p.id().to_string(),
        ActiveSource::Custom(cid) => cid.clone(),
    }
}

/// 返回当前激活主题的展示名称。
#[allow(dead_code)]
pub fn current_theme_label() -> String {
    let state = THEME.read().unwrap_or_else(PoisonError::into_inner);
    match &state.source {
        ActiveSource::Builtin(p) => p.label().to_string(),
        ActiveSource::Custom(cid) => state
            .custom_themes
            .iter()
            .find(|t| t.id == *cid)
            .map(|t| t.label.clone())
            .unwrap_or_else(|| cid.clone()),
    }
}

/// 返回当前是否开启实体背景铺底。
pub fn paint_bg() -> bool {
    THEME.read().unwrap_or_else(PoisonError::into_inner).paint_bg
}

/// 返回当前主题单调递增修订号（每次切换主题、开关透明或重载配置均递增）。
pub fn revision() -> u64 {
    THEME.read().unwrap_or_else(PoisonError::into_inner).revision
}

/// 返回当前生效的完整调色板快照。
pub fn palette() -> Palette {
    THEME.read().unwrap_or_else(PoisonError::into_inner).palette
}

fn build_preset_palette(preset: ThemePreset, paint_bg: bool) -> Palette {
    let mut p = preset.base_palette();
    if !paint_bg || preset == ThemePreset::Terminal {
        apply_transparent_mode(&mut p);
    }
    p
}

fn is_source_transparent_only(state: &ThemeState) -> bool {
    match &state.source {
        ActiveSource::Builtin(p) => *p == ThemePreset::Terminal,
        ActiveSource::Custom(cid) => state
            .custom_themes
            .iter()
            .find(|t| t.id == *cid)
            .is_some_and(|t| t.transparent),
    }
}

fn apply_source_by_id(
    state: &mut ThemeState,
    raw_id: &str,
    explicit_paint_bg: Option<bool>,
) -> Option<String> {
    let needle = raw_id.trim();
    if needle.is_empty() {
        return None;
    }
    let was_transparent_only = is_source_transparent_only(state);

    // 1. 优先匹配精确的自定义主题 ID（避免用户显式选择自定义主题被别名截胡）
    if let Some(ct) = state
        .custom_themes
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(needle))
        .cloned()
    {
        state.source = ActiveSource::Custom(ct.id.clone());
        state.paint_bg = if ct.transparent {
            false
        } else if let Some(bg) = explicit_paint_bg {
            bg
        } else if was_transparent_only {
            true
        } else {
            state.paint_bg
        };
        rebuild_active_palette(state);
        return Some(ct.label);
    }

    // 2. 匹配内置主题 ID 与常用别名
    if let Some(preset) = ThemePreset::parse(needle) {
        state.source = ActiveSource::Builtin(preset);
        state.paint_bg = if preset == ThemePreset::Terminal {
            false
        } else if let Some(bg) = explicit_paint_bg {
            bg
        } else if was_transparent_only {
            true
        } else {
            state.paint_bg
        };
        rebuild_active_palette(state);
        return Some(preset.label().to_string());
    }

    None
}

fn rebuild_active_palette(state: &mut ThemeState) {
    let mut p = match &state.source {
        ActiveSource::Builtin(preset) => preset.base_palette(),
        ActiveSource::Custom(cid) => state
            .custom_themes
            .iter()
            .find(|t| t.id == *cid)
            .map(|t| t.palette)
            .unwrap_or_else(|| ThemePreset::MiyuLavender.base_palette()),
    };
    if !state.paint_bg || is_source_transparent_only(state) {
        apply_transparent_mode(&mut p);
    }
    state.palette = p;
    state.revision = state.revision.wrapping_add(1);
}

pub fn text() -> Style {
    Style::new().fg(palette().text)
}
pub fn dim() -> Style {
    Style::new().fg(palette().dim)
}
pub fn faint() -> Style {
    Style::new().fg(palette().faint)
}
pub fn accent() -> Style {
    Style::new().fg(palette().accent)
}
pub fn accent_bold() -> Style {
    Style::new()
        .fg(palette().accent)
        .add_modifier(Modifier::BOLD)
}
pub fn picked() -> Style {
    Style::new()
        .fg(palette().picked)
        .add_modifier(Modifier::BOLD)
}
pub fn picked_bar() -> Style {
    let p = palette();
    Style::new()
        .bg(p.accent)
        .fg(p.on_accent)
        .add_modifier(Modifier::BOLD)
}
pub fn user_bar() -> Style {
    Style::new().fg(palette().accent)
}
pub fn thought() -> Style {
    Style::new().fg(palette().thought)
}
pub fn good() -> Style {
    Style::new().fg(palette().good)
}
pub fn warn() -> Style {
    Style::new().fg(palette().warn)
}
pub fn error() -> Style {
    Style::new().fg(palette().error)
}
pub fn added() -> Style {
    Style::new().fg(palette().added)
}
pub fn removed() -> Style {
    Style::new().fg(palette().removed)
}
pub fn side_title() -> Style {
    Style::new().fg(palette().side_title)
}
pub fn model() -> Style {
    Style::new().fg(palette().text).add_modifier(Modifier::BOLD)
}

// Markdown 语义样式
pub fn md_h1() -> Style {
    Style::new()
        .fg(palette().text)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}
pub fn md_heading() -> Style {
    Style::new()
        .fg(palette().md_heading)
        .add_modifier(Modifier::BOLD)
}
pub fn md_title() -> Style {
    Style::new().fg(palette().text).add_modifier(Modifier::BOLD)
}
pub fn md_bold() -> Style {
    Style::new()
        .fg(palette().md_bold)
        .add_modifier(Modifier::BOLD)
}
pub fn md_italic() -> Style {
    Style::new()
        .fg(palette().md_italic)
        .add_modifier(Modifier::ITALIC)
}
pub fn md_code() -> Style {
    Style::new().fg(palette().md_code)
}
pub fn md_link() -> Style {
    Style::new()
        .fg(palette().md_link)
        .add_modifier(Modifier::BOLD)
}
pub fn md_url() -> Style {
    Style::new().fg(palette().md_url)
}
pub fn md_image() -> Style {
    Style::new().fg(palette().md_image)
}
pub fn md_quote() -> Style {
    Style::new()
        .fg(palette().md_quote)
        .add_modifier(Modifier::ITALIC)
}
pub fn md_table_head() -> Style {
    Style::new()
        .fg(palette().md_table_head)
        .add_modifier(Modifier::BOLD)
}
pub fn md_list() -> Style {
    Style::new().fg(palette().accent)
}
pub fn md_math() -> Style {
    Style::new().fg(palette().md_math)
}

// 代码高亮语义样式
pub fn code_keyword() -> Style {
    Style::new().fg(palette().code_keyword)
}
pub fn code_function() -> Style {
    Style::new().fg(palette().code_function)
}
pub fn code_string() -> Style {
    Style::new().fg(palette().code_string)
}
pub fn code_number() -> Style {
    Style::new().fg(palette().code_number)
}
pub fn code_comment() -> Style {
    Style::new()
        .fg(palette().code_comment)
        .add_modifier(Modifier::ITALIC)
}
pub fn code_type() -> Style {
    Style::new().fg(palette().code_type)
}
pub fn code_constant() -> Style {
    Style::new().fg(palette().code_constant)
}
pub fn code_property() -> Style {
    Style::new().fg(palette().code_property)
}
pub fn code_operator() -> Style {
    Style::new().fg(palette().code_operator)
}
pub fn code_macro() -> Style {
    Style::new().fg(palette().code_macro)
}
pub fn code_parameter() -> Style {
    Style::new().fg(palette().code_parameter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_and_terminal_theme_clear_code_block_background() {
        let terminal = build_preset_palette(ThemePreset::Terminal, false);
        assert_eq!(terminal.background, Color::Reset);
        assert_eq!(terminal.surface, Color::Reset);
        assert_eq!(terminal.panel_bg, Color::Reset);
        assert_eq!(terminal.md_code_bg, Color::Reset);

        let transparent_lavender = build_preset_palette(ThemePreset::MiyuLavender, false);
        assert_eq!(transparent_lavender.background, Color::Reset);
        assert_eq!(transparent_lavender.surface, Color::Reset);
        assert_eq!(transparent_lavender.panel_bg, Color::Reset);
        assert_eq!(transparent_lavender.md_code_bg, Color::Reset);

        let opaque_lavender = build_preset_palette(ThemePreset::MiyuLavender, true);
        assert_ne!(opaque_lavender.background, Color::Reset);
        assert_ne!(opaque_lavender.surface, Color::Reset);
        assert_ne!(opaque_lavender.panel_bg, Color::Reset);
        assert_ne!(opaque_lavender.md_code_bg, Color::Reset);
    }

    #[test]
    fn miyu_paper_and_expanded_builtin_presets_parse_with_distinct_palettes() {
        assert_eq!(ThemePreset::parse("miyu-paper"), Some(ThemePreset::MiyuPaper));
        assert_eq!(ThemePreset::parse("paper"), Some(ThemePreset::MiyuPaper));
        assert_eq!(ThemePreset::parse("warm-light"), Some(ThemePreset::MiyuPaper));
        assert_eq!(ThemePreset::parse("mocha"), Some(ThemePreset::CatppuccinMocha));
        assert_eq!(ThemePreset::parse("rose"), Some(ThemePreset::RosePine));
        assert_eq!(ThemePreset::parse("kanagawa"), Some(ThemePreset::KanagawaWave));
        assert_eq!(ThemePreset::parse("nord"), Some(ThemePreset::Nord));
        assert_eq!(ThemePreset::parse("everforest"), Some(ThemePreset::EverforestDark));
        assert_eq!(ThemePreset::parse("gruvbox"), Some(ThemePreset::GruvboxDark));
        assert_eq!(ThemePreset::parse("dracula"), Some(ThemePreset::Dracula));
        assert_eq!(ThemePreset::parse("latte"), Some(ThemePreset::CatppuccinLatte));
        assert_eq!(ThemePreset::ALL.len(), 14);

        let paper = build_preset_palette(ThemePreset::MiyuPaper, true);
        assert_eq!(paper.background, Color::Rgb(0xf6, 0xf2, 0xea));
        assert_eq!(paper.text, Color::Rgb(0x2b, 0x27, 0x23));
        assert_ne!(paper.background, build_preset_palette(ThemePreset::MiyuDay, true).background);
        assert_ne!(
            build_preset_palette(ThemePreset::CatppuccinMocha, true).background,
            build_preset_palette(ThemePreset::RosePine, true).background
        );
    }

    #[test]
    fn custom_theme_supports_seed_derivation_partial_override_and_export() {
        let dir = std::env::temp_dir().join(format!("agy_tui_themes_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 1. 种子派生 + 局部颜色覆写（含 #RGB 短写与 kebab-case 字段名）
        let ocean_json = r##"{
            "name": "Deep Ocean",
            "base": "catppuccin-mocha",
            "seed": "#38bdf8",
            "colors": {
                "background": "#0f172a",
                "accent": "#0ef",
                "code-keyword": "#f472b6"
            }
        }"##;
        std::fs::write(dir.join("ocean.json"), ocean_json).unwrap();

        // 2. 损坏的 JSON 文件应被静默跳过，不影响正常主题加载
        std::fs::write(dir.join("broken.json"), "{ not valid json").unwrap();

        let loaded = load_custom_themes_from_dir(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "ocean");
        assert_eq!(loaded[0].label, "Deep Ocean");
        assert_eq!(loaded[0].palette.background, Color::Rgb(0x0f, 0x17, 0x2a));
        assert_eq!(loaded[0].palette.accent, Color::Rgb(0x00, 0xee, 0xff));
        assert_eq!(loaded[0].palette.code_keyword, Color::Rgb(0xf4, 0x72, 0xb6));

        // 3. 导出模板后再重新加载，应完整还原所有颜色槽
        let exported = export_theme_template(
            &dir,
            "My Exported",
            "rose-pine",
            &ThemePreset::RosePine.base_palette(),
            false,
        )
        .unwrap();
        assert!(exported.ends_with("my-exported.json"));
        let reloaded = load_custom_themes_from_dir(&dir);
        assert_eq!(reloaded.len(), 2);
        let exp = reloaded.iter().find(|t| t.id == "my-exported").unwrap();
        assert_eq!(exp.palette.background, ThemePreset::RosePine.base_palette().background);
        assert_eq!(exp.palette.accent, ThemePreset::RosePine.base_palette().accent);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_picker_previews_live_renders_swatches_and_restores_on_esc() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use unicode_width::UnicodeWidthStr;

        init(ThemePreset::MiyuLavender, true);
        let orig_id = current_theme_id();
        let orig_rev = revision();

        let mut app = crate::app::App::new(None, "/tmp/agy-tui".into(), None, true, true);
        app.editor.set_text("/theme");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.picker.is_some(), "/theme should open picker");

        // 1. 渲染主题选择器，确认内置新主题与色板方块均已绘制
        let mut term = Terminal::new(TestBackend::new(100, 32)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let buf = term.backend().buffer();
        let mut rendered = String::new();
        for y in 0..32u16 {
            let mut x = 0u16;
            while x < 100 {
                let sym = buf[(x, y)].symbol();
                rendered.push_str(sym);
                x += u16::try_from(sym.width()).unwrap_or(1).max(1);
            }
            rendered.push('\n');
        }
        assert!(rendered.contains("Catppuccin Mocha"), "missing new theme:\n{rendered}");
        assert!(rendered.contains("Rosé Pine"), "missing rose pine:\n{rendered}");
        assert!(rendered.contains('■'), "missing theme swatch blocks:\n{rendered}");

        // 2. 下移光标触发实时预览，主题 ID 与 revision 立即变化
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_ne!(current_theme_id(), orig_id);
        assert!(revision() > orig_rev);

        // 3. 按 Esc 取消选择器，应精确还原原主题
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.picker.is_none());
        assert_eq!(current_theme_id(), orig_id);
    }
}
