//! 用户自定义主题解析、目录扫描与模板导出模块。
//!
//! 配置文件存放于 `$XDG_CONFIG_HOME/agy-tui/themes/*.json`（Windows 为 `%APPDATA%\agy-tui\themes\*.json`）。
//! 支持三种定制方式：
//! 1. 单色派生：仅指定 `base` + `seed`（如 `"#38bdf8"`），经 Material Design 3 HCT 自动生成 44 色。
//! 2. 局部微调：指定 `base` + `colors` 中的任意子集，其余槽位自动继承基底主题。
//! 3. 全量手调：在 `colors` 中完整定义全部 44 个颜色槽。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use super::palette::{Palette, apply_transparent_mode, md3_overlay};
use super::presets::ThemePreset;

/// 已解析加载的用户自定义主题。
#[derive(Debug, Clone)]
pub struct CustomTheme {
    pub id: String,
    pub label: String,
    pub base_preset: ThemePreset,
    pub transparent: bool,
    pub palette: Palette,
    #[allow(dead_code)]
    pub source_path: PathBuf,
}

/// 自定义主题 JSON 文件反序列化结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomThemeFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparent: Option<bool>,
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
}

/// 解析十六进制或特殊关键词颜色字符串（支持 `#RRGGBB`、`#RGB`、`RRGGBB`、`transparent`、`reset`）。
///
/// 参数:
/// - `raw`: 原始颜色文本
///
/// 返回:
/// - 合法颜色返回 `Some(Color)`，非法格式返回 `None`
pub fn parse_color(raw: &str) -> Option<Color> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("reset")
        || trimmed.eq_ignore_ascii_case("transparent")
        || trimmed.eq_ignore_ascii_case("none")
    {
        return Some(Color::Reset);
    }
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let bytes = hex.as_bytes();
    if bytes.len() == 3 && bytes.iter().all(u8::is_ascii_hexdigit) {
        let expand = |b: u8| -> Option<u8> {
            let v = (b as char).to_digit(16)? as u8;
            Some((v << 4) | v)
        };
        return Some(Color::Rgb(expand(bytes[0])?, expand(bytes[1])?, expand(bytes[2])?));
    }
    if bytes.len() == 6 && bytes.iter().all(u8::is_ascii_hexdigit) {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(r, g, b));
    }
    None
}

/// 将 `Color` 格式化为标准十六进制或 `"reset"` 字符串（用于模板导出）。
///
/// 参数:
/// - `color`: 待序列化的颜色值
///
/// 返回:
/// - `#rrggbb` 或 `"reset"`
pub fn format_color(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "reset".to_string(),
    }
}

/// 将颜色值转为 24 位 RGB 种子整数（供 MD3 HCT 衍生使用）。
fn parse_seed_rgb(raw: &str) -> Option<u32> {
    match parse_color(raw)? {
        Color::Rgb(r, g, b) => Some((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)),
        _ => None,
    }
}

/// 清洗并规范化自定义主题 ID（转小写，非字母数字字符折叠为 `-`）。
///
/// 参数:
/// - `raw`: 原始名称或文件名
///
/// 返回:
/// - 规范化后的 kebab-case 标识，无效时返回空字符串
pub fn sanitize_theme_id(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch == '-' || ch == '_' || ch == ' ') && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// 从指定目录扫描并解析全部 `*.json` 自定义主题文件。
///
/// 参数:
/// - `dir`: 自定义主题目录路径
///
/// 返回:
/// - 解析成功且按 ID 升序排列的 `Vec<CustomTheme>`；目录不存在或单文件损坏时静默跳过
pub fn load_custom_themes_from_dir(dir: &Path) -> Vec<CustomTheme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        })
        .collect();
    files.sort();

    let mut themes = Vec::new();
    for path in files {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(theme) = parse_custom_theme(&raw, &path) {
            if !themes.iter().any(|existing: &CustomTheme| existing.id == theme.id) {
                themes.push(theme);
            }
        }
    }
    themes.sort_by(|a, b| a.id.cmp(&b.id));
    themes
}

/// 将单份 JSON 文本编译为完整的 `CustomTheme`。
///
/// 参数:
/// - `raw`: JSON 文件内容
/// - `path`: 来源文件路径（用于缺省 ID/名称推断）
///
/// 返回:
/// - 解析有效返回 `Some(CustomTheme)`，格式错误返回 `None`
pub fn parse_custom_theme(raw: &str, path: &Path) -> Option<CustomTheme> {
    let spec: CustomThemeFile = serde_json::from_str(raw).ok()?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("custom");
    // 1. 归一化主题 ID，若与内置主题 ID 撞名则自动附加 `custom-` 前缀避免覆盖内置预设
    let mut id = sanitize_theme_id(spec.id.as_deref().unwrap_or(stem));
    if id.is_empty() {
        id = "custom".to_string();
    }
    if ThemePreset::ALL.iter().any(|p| p.id() == id) {
        id = format!("custom-{id}");
    }

    // 2. 选择基底预设并在指定 `seed` 时通过 MD3 HCT 自动派生整套 44 色
    let base_preset = spec
        .base
        .as_deref()
        .and_then(ThemePreset::parse)
        .unwrap_or(ThemePreset::MiyuNight);
    let mut palette = base_preset.base_palette();
    if let Some(seed_raw) = spec.seed.as_deref()
        && let Some(seed) = parse_seed_rgb(seed_raw)
    {
        let dark = spec.dark.unwrap_or_else(|| base_preset.is_dark());
        palette = md3_overlay(&palette, seed, dark);
    }

    // 3. 逐槽应用 `colors` 局部覆写（兼容 snake_case 与 kebab-case 字段名）
    for (key, value) in &spec.colors {
        if let Some(color) = parse_color(value) {
            apply_color_slot(&mut palette, key, color);
        }
    }

    let transparent = spec
        .transparent
        .unwrap_or(base_preset == ThemePreset::Terminal || palette.background == Color::Reset);
    if transparent {
        apply_transparent_mode(&mut palette);
    }

    let display_name = spec
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| id.clone());

    Some(CustomTheme {
        id,
        label: display_name,
        base_preset,
        transparent,
        palette,
        source_path: path.to_path_buf(),
    })
}

/// 将当前调色板导出为可直接编辑的自定义主题 JSON 模板文件。
///
/// 参数:
/// - `dir`: 自定义主题存放目录
/// - `raw_name`: 用户指定的模板名称（空字符串时默认 `"custom"`）
/// - `base_id`: 参考基底主题 ID
/// - `palette`: 当前激活的 44 色调色板
/// - `transparent`: 当前是否启用透明背景
///
/// 返回:
/// - 写入成功返回生成文件的绝对路径，失败返回可读错误说明
pub fn export_theme_template(
    dir: &Path,
    raw_name: &str,
    base_id: &str,
    palette: &Palette,
    transparent: bool,
) -> Result<PathBuf, String> {
    let slug = sanitize_theme_id(if raw_name.trim().is_empty() {
        "custom"
    } else {
        raw_name
    });
    if slug.is_empty() {
        return Err("主题名称仅支持字母、数字、连字符或下划线".to_string());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建主题目录失败：{e}"))?;

    let mut colors = BTreeMap::new();
    for (slot, color) in palette_slots(palette) {
        colors.insert(slot.to_string(), format_color(color));
    }
    let file = CustomThemeFile {
        id: Some(slug.clone()),
        name: Some(format!("{slug} (自定义)")),
        base: Some(base_id.to_string()),
        seed: None,
        dark: None,
        transparent: Some(transparent),
        colors,
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| format!("序列化主题模板失败：{e}"))?;
    let target = dir.join(format!("{slug}.json"));
    std::fs::write(&target, format!("{json}\n")).map_err(|e| format!("写入主题文件失败：{e}"))?;
    Ok(target)
}

/// 按键名将单个自定义颜色写入调色板对应槽位（兼容 snake_case / kebab-case 与常见简写）。
fn apply_color_slot(p: &mut Palette, key: &str, color: Color) {
    let norm = key.trim().to_ascii_lowercase().replace('-', "_");
    match norm.as_str() {
        "background" | "bg" => p.background = color,
        "surface" => p.surface = color,
        "panel_bg" | "panel" => p.panel_bg = color,
        "text" | "fg" | "foreground" => p.text = color,
        "dim" => p.dim = color,
        "faint" | "muted" => p.faint = color,
        "accent" | "primary" => p.accent = color,
        "picked" | "secondary" => p.picked = color,
        "on_accent" => p.on_accent = color,
        "thought" => p.thought = color,
        "good" | "success" => p.good = color,
        "warn" | "warning" => p.warn = color,
        "error" | "danger" => p.error = color,
        "added" => p.added = color,
        "removed" => p.removed = color,
        "diff_added_bg" => p.diff_added_bg = color,
        "diff_removed_bg" => p.diff_removed_bg = color,
        "diff_added_emphasis_bg" => p.diff_added_emphasis_bg = color,
        "diff_removed_emphasis_bg" => p.diff_removed_emphasis_bg = color,
        "diff_gutter_fg" => p.diff_gutter_fg = color,
        "md_bold" => p.md_bold = color,
        "md_italic" => p.md_italic = color,
        "md_code" => p.md_code = color,
        "md_code_bg" => p.md_code_bg = color,
        "md_link" => p.md_link = color,
        "md_url" => p.md_url = color,
        "md_image" => p.md_image = color,
        "md_quote" => p.md_quote = color,
        "md_table_head" => p.md_table_head = color,
        "md_heading" => p.md_heading = color,
        "md_math" => p.md_math = color,
        "side_title" => p.side_title = color,
        "row_focus_bg" | "hover_bg" => p.row_focus_bg = color,
        "code_keyword" => p.code_keyword = color,
        "code_function" => p.code_function = color,
        "code_string" => p.code_string = color,
        "code_number" => p.code_number = color,
        "code_comment" => p.code_comment = color,
        "code_type" => p.code_type = color,
        "code_constant" => p.code_constant = color,
        "code_property" => p.code_property = color,
        "code_operator" => p.code_operator = color,
        "code_macro" => p.code_macro = color,
        "code_parameter" => p.code_parameter = color,
        _ => {}
    }
}

/// 枚举调色板全部 44 个标准槽位名与对应色值（用于完整模板导出）。
fn palette_slots(p: &Palette) -> [(&'static str, Color); 44] {
    [
        ("background", p.background),
        ("surface", p.surface),
        ("panel_bg", p.panel_bg),
        ("text", p.text),
        ("dim", p.dim),
        ("faint", p.faint),
        ("accent", p.accent),
        ("picked", p.picked),
        ("on_accent", p.on_accent),
        ("thought", p.thought),
        ("good", p.good),
        ("warn", p.warn),
        ("error", p.error),
        ("added", p.added),
        ("removed", p.removed),
        ("diff_added_bg", p.diff_added_bg),
        ("diff_removed_bg", p.diff_removed_bg),
        ("diff_added_emphasis_bg", p.diff_added_emphasis_bg),
        ("diff_removed_emphasis_bg", p.diff_removed_emphasis_bg),
        ("diff_gutter_fg", p.diff_gutter_fg),
        ("md_bold", p.md_bold),
        ("md_italic", p.md_italic),
        ("md_code", p.md_code),
        ("md_code_bg", p.md_code_bg),
        ("md_link", p.md_link),
        ("md_url", p.md_url),
        ("md_image", p.md_image),
        ("md_quote", p.md_quote),
        ("md_table_head", p.md_table_head),
        ("md_heading", p.md_heading),
        ("md_math", p.md_math),
        ("side_title", p.side_title),
        ("row_focus_bg", p.row_focus_bg),
        ("code_keyword", p.code_keyword),
        ("code_function", p.code_function),
        ("code_string", p.code_string),
        ("code_number", p.code_number),
        ("code_comment", p.code_comment),
        ("code_type", p.code_type),
        ("code_constant", p.code_constant),
        ("code_property", p.code_property),
        ("code_operator", p.code_operator),
        ("code_macro", p.code_macro),
        ("code_parameter", p.code_parameter),
    ]
}
