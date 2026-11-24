//! 主题与配色引擎（参考 LiteMiyu `theme/` + Material Design 3 `SchemeContent` 衍生调色板）。

use std::sync::{PoisonError, RwLock};

use material_colors::blend::harmonize;
use material_colors::color::Rgb as MdRgb;
use material_colors::hct::Hct;
use material_colors::palette::TonalPalette;
use material_colors::scheme::variant::SchemeContent;
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePreset {
    /// LiteMiyu 经典暗夜紫蓝（Tokyo Night 基底）
    MiyuNight,
    /// LiteMiyu 薰衣草莫兰迪（MD3 柔和紫灰衍生，贴近官方截图氛围）
    MiyuLavender,
    /// 鼠尾草森林暗色（MD3 柔和绿蓝衍生）
    MiyuForest,
    /// 浅色护眼日间主题（Tokyo Night Day）
    MiyuDay,
    /// 透明继承宿主终端背景
    Terminal,
}

impl ThemePreset {
    pub const ALL: &'static [ThemePreset] = &[
        ThemePreset::MiyuLavender,
        ThemePreset::MiyuNight,
        ThemePreset::MiyuForest,
        ThemePreset::MiyuDay,
        ThemePreset::Terminal,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::MiyuLavender => "miyu-lavender",
            Self::MiyuNight => "miyu-night",
            Self::MiyuForest => "miyu-forest",
            Self::MiyuDay => "miyu-day",
            Self::Terminal => "terminal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MiyuLavender => "Miyu Lavender (莫兰迪紫蓝·默认)",
            Self::MiyuNight => "Miyu Night (Tokyo Night 深色)",
            Self::MiyuForest => "Miyu Forest (MD3 鼠尾草暗色)",
            Self::MiyuDay => "Miyu Day (柔和浅色)",
            Self::Terminal => "Terminal (透明跟随终端)",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "miyu-lavender" | "lavender" | "miyu" => Some(Self::MiyuLavender),
            "miyu-night" | "night" | "tokyonight" | "tokyo-night" | "dark" => Some(Self::MiyuNight),
            "miyu-forest" | "forest" | "green" => Some(Self::MiyuForest),
            "miyu-day" | "day" | "light" => Some(Self::MiyuDay),
            "terminal" | "transparent" | "native" => Some(Self::Terminal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub panel_bg: Color,
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub picked: Color,
    pub on_accent: Color,
    pub thought: Color,
    pub good: Color,
    pub warn: Color,
    pub error: Color,
    pub added: Color,
    pub removed: Color,
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
    pub diff_added_emphasis_bg: Color,
    pub diff_removed_emphasis_bg: Color,
    pub diff_gutter_fg: Color,
    pub md_bold: Color,
    pub md_italic: Color,
    pub md_code: Color,
    pub md_code_bg: Color,
    pub md_link: Color,
    pub md_url: Color,
    pub md_image: Color,
    pub md_quote: Color,
    pub md_table_head: Color,
    pub md_heading: Color,
    pub md_math: Color,
    pub side_title: Color,
    pub row_focus_bg: Color,
    pub code_keyword: Color,
    pub code_function: Color,
    pub code_string: Color,
    pub code_number: Color,
    pub code_comment: Color,
    pub code_type: Color,
    pub code_constant: Color,
    pub code_property: Color,
    pub code_operator: Color,
    pub code_macro: Color,
    pub code_parameter: Color,
}

const NIGHT: Palette = Palette {
    background: Color::Rgb(0x1a, 0x1b, 0x26),
    surface: Color::Rgb(0x20, 0x23, 0x33),
    panel_bg: Color::Rgb(0x24, 0x28, 0x3b),
    text: Color::Rgb(0xc0, 0xca, 0xf5),
    dim: Color::Rgb(0x82, 0x8b, 0xb8),
    faint: Color::Rgb(0x56, 0x5f, 0x89),
    accent: Color::Rgb(0x7a, 0xa2, 0xf7),
    picked: Color::Rgb(0xbb, 0x9a, 0xf7),
    on_accent: Color::Rgb(0x1a, 0x1b, 0x26),
    thought: Color::Rgb(0x41, 0xa6, 0xb5),
    good: Color::Rgb(0x9e, 0xce, 0x6a),
    warn: Color::Rgb(0xe0, 0xaf, 0x68),
    error: Color::Rgb(0xf7, 0x76, 0x8e),
    added: Color::Rgb(0x9e, 0xce, 0x6a),
    removed: Color::Rgb(0xf7, 0x76, 0x8e),
    diff_added_bg: Color::Rgb(0x1b, 0x2e, 0x28),
    diff_removed_bg: Color::Rgb(0x36, 0x1f, 0x28),
    diff_added_emphasis_bg: Color::Rgb(0x28, 0x4c, 0x3b),
    diff_removed_emphasis_bg: Color::Rgb(0x5c, 0x2c, 0x3b),
    diff_gutter_fg: Color::Rgb(0x9a, 0xa5, 0xce),
    md_bold: Color::Rgb(0xe5, 0xa0, 0x7a),
    md_italic: Color::Rgb(0xa9, 0xb1, 0xd6),
    md_code: Color::Rgb(0x7d, 0xcf, 0xff),
    md_code_bg: Color::Rgb(0x24, 0x28, 0x3b),
    md_link: Color::Rgb(0x7a, 0xa2, 0xf7),
    md_url: Color::Rgb(0x6a, 0x8f, 0xd8),
    md_image: Color::Rgb(0x9d, 0x7c, 0xd8),
    md_quote: Color::Rgb(0x82, 0x8b, 0xb8),
    md_table_head: Color::Rgb(0x7a, 0xa2, 0xf7),
    md_heading: Color::Rgb(0xbb, 0x9a, 0xf7),
    md_math: Color::Rgb(0xc0, 0xca, 0xf5),
    side_title: Color::Rgb(0x7a, 0xa2, 0xf7),
    row_focus_bg: Color::Rgb(0x2e, 0x35, 0x52),
    code_keyword: Color::Rgb(0xbb, 0x9a, 0xf7),
    code_function: Color::Rgb(0x7a, 0xa2, 0xf7),
    code_string: Color::Rgb(0x9e, 0xce, 0x6a),
    code_number: Color::Rgb(0xff, 0x9e, 0x64),
    code_comment: Color::Rgb(0x56, 0x5f, 0x89),
    code_type: Color::Rgb(0x2a, 0xc3, 0xde),
    code_constant: Color::Rgb(0xff, 0x9e, 0x64),
    code_property: Color::Rgb(0x73, 0xda, 0xca),
    code_operator: Color::Rgb(0x89, 0xdd, 0xff),
    code_macro: Color::Rgb(0x7d, 0xcf, 0xff),
    code_parameter: Color::Rgb(0xe0, 0xaf, 0x68),
};

const DAY: Palette = Palette {
    background: Color::Rgb(0xe8, 0xe9, 0xee),
    surface: Color::Rgb(0xdf, 0xe1, 0xe8),
    panel_bg: Color::Rgb(0xd5, 0xd8, 0xe2),
    text: Color::Rgb(0x2c, 0x36, 0x5e),
    dim: Color::Rgb(0x61, 0x69, 0x92),
    faint: Color::Rgb(0x96, 0x9c, 0xba),
    accent: Color::Rgb(0x2e, 0x7d, 0xe9),
    picked: Color::Rgb(0x98, 0x54, 0xf1),
    on_accent: Color::Rgb(0xf5, 0xf6, 0xfa),
    thought: Color::Rgb(0x00, 0x71, 0x97),
    good: Color::Rgb(0x58, 0x75, 0x39),
    warn: Color::Rgb(0x8c, 0x6c, 0x3e),
    error: Color::Rgb(0xd2, 0x26, 0x56),
    added: Color::Rgb(0x58, 0x75, 0x39),
    removed: Color::Rgb(0xd2, 0x26, 0x56),
    diff_added_bg: Color::Rgb(0xdc, 0xf2, 0xe4),
    diff_removed_bg: Color::Rgb(0xf8, 0xdc, 0xe0),
    diff_added_emphasis_bg: Color::Rgb(0xb2, 0xe2, 0xc3),
    diff_removed_emphasis_bg: Color::Rgb(0xf0, 0xb6, 0xc0),
    diff_gutter_fg: Color::Rgb(0x56, 0x60, 0x82),
    md_bold: Color::Rgb(0xb1, 0x5c, 0x00),
    md_italic: Color::Rgb(0x61, 0x72, 0xb0),
    md_code: Color::Rgb(0x18, 0x80, 0x92),
    md_code_bg: Color::Rgb(0xd8, 0xdc, 0xe8),
    md_link: Color::Rgb(0x2e, 0x7d, 0xe9),
    md_url: Color::Rgb(0x2e, 0x7d, 0xe9),
    md_image: Color::Rgb(0x78, 0x47, 0xbd),
    md_quote: Color::Rgb(0x68, 0x70, 0x9a),
    md_table_head: Color::Rgb(0x2e, 0x7d, 0xe9),
    md_heading: Color::Rgb(0x98, 0x54, 0xf1),
    md_math: Color::Rgb(0x37, 0x60, 0xbf),
    side_title: Color::Rgb(0x2e, 0x7d, 0xe9),
    row_focus_bg: Color::Rgb(0xc2, 0xcb, 0xe6),
    code_keyword: Color::Rgb(0x98, 0x54, 0xf1),
    code_function: Color::Rgb(0x2e, 0x7d, 0xe9),
    code_string: Color::Rgb(0x58, 0x75, 0x39),
    code_number: Color::Rgb(0xb1, 0x5c, 0x00),
    code_comment: Color::Rgb(0x84, 0x8c, 0xb5),
    code_type: Color::Rgb(0x18, 0x80, 0x92),
    code_constant: Color::Rgb(0xb1, 0x5c, 0x00),
    code_property: Color::Rgb(0x38, 0x70, 0x68),
    code_operator: Color::Rgb(0x00, 0x6a, 0x83),
    code_macro: Color::Rgb(0x00, 0x71, 0x97),
    code_parameter: Color::Rgb(0x8c, 0x6c, 0x3e),
};

/// Material Design 3 衍生调色板（复刻 LiteMiyu `md3.rs`）：保留 Tokyo Night 的明暗对比层次，只柔和替换色相。
fn md3_overlay(base: &Palette, seed: u32, dark: bool) -> Palette {
    let source = MdRgb::from_u32(seed);
    let scheme = SchemeContent::new(Hct::new(source), dark, Some(0.0)).scheme;
    let neutral = &scheme.neutral_palette;
    let variant = &scheme.neutral_variant_palette;
    let primary = &scheme.primary_palette;
    let second = &scheme.secondary_palette;
    let third = &scheme.tertiary_palette;

    let like = |from: &TonalPalette, color: Color| match color {
        Color::Rgb(r, g, b) => {
            let tone = Hct::new(MdRgb::new(r, g, b)).get_tone();
            let c = MdRgb::from(from.get_hct(tone));
            Color::Rgb(c.red, c.green, c.blue)
        }
        other => other,
    };
    let near = |color: Color| match color {
        Color::Rgb(r, g, b) => {
            let c = harmonize(MdRgb::new(r, g, b), source);
            Color::Rgb(c.red, c.green, c.blue)
        }
        other => other,
    };

    Palette {
        background: like(neutral, base.background),
        surface: like(neutral, base.surface),
        panel_bg: like(neutral, base.panel_bg),
        text: like(neutral, base.text),
        dim: like(variant, base.dim),
        faint: like(variant, base.faint),
        accent: like(primary, base.accent),
        picked: like(third, base.picked),
        on_accent: like(neutral, base.on_accent),
        thought: like(second, base.thought),
        good: near(base.good),
        warn: near(base.warn),
        error: near(base.error),
        added: near(base.added),
        removed: near(base.removed),
        diff_added_bg: near(base.diff_added_bg),
        diff_removed_bg: near(base.diff_removed_bg),
        diff_added_emphasis_bg: near(base.diff_added_emphasis_bg),
        diff_removed_emphasis_bg: near(base.diff_removed_emphasis_bg),
        diff_gutter_fg: like(variant, base.diff_gutter_fg),
        md_bold: like(primary, base.md_bold),
        md_italic: like(variant, base.md_italic),
        md_code: like(third, base.md_code),
        md_code_bg: like(neutral, base.md_code_bg),
        md_link: like(primary, base.md_link),
        md_url: like(primary, base.md_url),
        md_image: like(third, base.md_image),
        md_quote: like(variant, base.md_quote),
        md_table_head: like(primary, base.md_table_head),
        md_heading: near(base.md_heading),
        md_math: like(neutral, base.md_math),
        side_title: like(primary, base.side_title),
        row_focus_bg: like(primary, base.row_focus_bg),
        code_keyword: near(base.code_keyword),
        code_function: near(base.code_function),
        code_string: near(base.code_string),
        code_number: near(base.code_number),
        code_comment: like(variant, base.code_comment),
        code_type: near(base.code_type),
        code_constant: near(base.code_constant),
        code_property: near(base.code_property),
        code_operator: like(variant, base.code_operator),
        code_macro: near(base.code_macro),
        code_parameter: near(base.code_parameter),
    }
}

fn build_palette(preset: ThemePreset, paint_bg: bool) -> Palette {
    let mut p = match preset {
        ThemePreset::MiyuLavender => md3_overlay(&NIGHT, 0x8b7cf6, true),
        ThemePreset::MiyuNight => NIGHT,
        ThemePreset::MiyuForest => md3_overlay(&NIGHT, 0x4ca87a, true),
        ThemePreset::MiyuDay => DAY,
        ThemePreset::Terminal => {
            let mut t = md3_overlay(&NIGHT, 0x8b7cf6, true);
            t.background = Color::Reset;
            t
        }
    };
    if !paint_bg {
        p.background = Color::Reset;
    }
    p
}

struct ThemeState {
    preset: ThemePreset,
    paint_bg: bool,
    palette: Palette,
}

static THEME: RwLock<ThemeState> = RwLock::new(ThemeState {
    preset: ThemePreset::MiyuLavender,
    paint_bg: true,
    palette: NIGHT,
});

pub fn init(preset: ThemePreset, paint_bg: bool) {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    state.preset = preset;
    state.paint_bg = paint_bg;
    state.palette = build_palette(preset, paint_bg);
}

pub fn set_preset(preset: ThemePreset) {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    state.preset = preset;
    if preset == ThemePreset::Terminal {
        state.paint_bg = false;
    }
    state.palette = build_palette(state.preset, state.paint_bg);
}

pub fn toggle_bg() -> bool {
    let mut state = THEME.write().unwrap_or_else(PoisonError::into_inner);
    state.paint_bg = !state.paint_bg;
    state.palette = build_palette(state.preset, state.paint_bg);
    state.paint_bg
}

pub fn current_preset() -> ThemePreset {
    THEME.read().unwrap_or_else(PoisonError::into_inner).preset
}

pub fn paint_bg() -> bool {
    THEME.read().unwrap_or_else(PoisonError::into_inner).paint_bg
}

pub fn palette() -> Palette {
    THEME.read().unwrap_or_else(PoisonError::into_inner).palette
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
