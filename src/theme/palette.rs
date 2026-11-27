//! 调色板核心数据结构、Material Design 3 衍生算法与透明模式调优工具。

use material_colors::blend::harmonize;
use material_colors::color::Rgb as MdRgb;
use material_colors::hct::Hct;
use material_colors::palette::TonalPalette;
use material_colors::scheme::variant::SchemeContent;
use ratatui::style::Color;

/// 终端界面与代码语法高亮的完整 44 槽调色板。
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

/// Material Design 3 衍生调色板（复刻 LiteMiyu `md3.rs`）：保留基底明暗对比层次，柔和替换色相。
///
/// 参数:
/// - `base`: 提供明度阶梯的基底调色板
/// - `seed`: 24 位 RGB 种子色（如 `0x8b7cf6`）
/// - `dark`: 是否按暗色模式构建 MD3 `SchemeContent`
///
/// 返回:
/// - 衍生后的完整 `Palette`
pub fn md3_overlay(base: &Palette, seed: u32, dark: bool) -> Palette {
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

/// 对调色板应用透明背景模式：清除铺底色，并适度提亮弱前景色避免融入宿主终端背景。
///
/// 参数:
/// - `p`: 待就地调整的调色板
pub fn apply_transparent_mode(p: &mut Palette) {
    p.background = Color::Reset;
    p.surface = Color::Reset;
    p.panel_bg = Color::Reset;
    p.md_code_bg = Color::Reset;
    p.faint = blend_rgb(p.faint, p.dim, 55);
    p.code_comment = blend_rgb(p.code_comment, p.dim, 55);
}

/// 将颜色 `a` 按百分比 `pct` 向颜色 `b` 柔和混合（用于透明模式下提升弱色槽可读性）。
///
/// 参数:
/// - `a`: 起始颜色
/// - `b`: 目标混合颜色
/// - `pct`: 混合占比（0..=100）
///
/// 返回:
/// - 混合后的颜色；若任一颜色非 `Color::Rgb` 则原样返回 `a`
pub fn blend_rgb(a: Color, b: Color, pct: u16) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let p = pct.min(100);
            let q = 100 - p;
            let mix = |x: u8, y: u8| ((u16::from(x) * q + u16::from(y) * p) / 100) as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => a,
    }
}

/// 提取用于 `/theme` 选择器右侧展示的 5 色代表色板（底色、强调、高亮、成功、警示）。
///
/// 参数:
/// - `p`: 目标主题的原始调色板
///
/// 返回:
/// - 包含 5 个代表色的固定数组
pub fn swatch_colors(p: &Palette) -> [Color; 5] {
    let base = if p.background == Color::Reset {
        p.dim
    } else {
        p.background
    };
    [base, p.accent, p.picked, p.good, p.error]
}
