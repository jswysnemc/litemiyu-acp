//! 底部状态行实时动效（借鉴 `jswysnemc/sai` 的 `activity_animation` 与 `work_status`）：
//! 1. 左侧固定 5 列等宽的会眨眼、会偶尔换神情的小表情 `(◕‿◕)` / `(-‿-)` / `(◔‿◔)` / `(∙‿∙)` / `(^‿^)`
//! 2. 中间采用 2 秒周期余弦扫描带的真彩色流光文字，适配深色/浅色/透明主题
//! 3. 右侧跟进弱化的整数秒耗时详情（`0s` / `12s` / `1m 05s`），按 32ms 统一时钟驱动

use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme;
use crate::transcript::{EntryKind, Transcript};

/// 状态动效统一刷新节拍（与 `sai` 保持一致：32ms / 帧 ≈ 31.25fps）。
pub const ACTIVITY_FRAME_INTERVAL: Duration = Duration::from_millis(32);

// ── 1. 左侧状态伴随表情（固定 5 列宽度，眨眼优先于神情切换） ──

/// 常态表情，神情排期中出现最多。
pub const DEFAULT_FACE: &str = "(◕‿◕)";
/// 眨眼表情。
pub const BLINK_FACE: &str = "(-‿-)";
/// 偶尔切换的神情；均采用常用编程字体收录且列宽固定的字符，切换时不抖动排版。
pub const GLANCE_FACES: [&str; 3] = ["(◔‿◔)", "(∙‿∙)", "(^‿^)"];

/// 两次眨眼之间的最短与浮动间隔（按 32ms/帧约为 1.3 ~ 2.6 秒）。
const BLINK_GAP_MIN: usize = 40;
const BLINK_GAP_SPREAD: usize = 40;
/// 每次眨眼持续约 0.1 秒。
const BLINK_FRAMES: usize = 3;
/// 每段神情持续约 1 ~ 3 秒。
const MOOD_MIN: usize = 30;
const MOOD_SPREAD: usize = 64;
/// 排期分段数（周期约半分钟，重复不易察觉且每帧开销固定）。
const SCHEDULE_SEGMENTS: u32 = 24;
const BLINK_SALT: u32 = 0x5bd1;
const MOOD_SALT: u32 = 0x27d4;

// ── 2. 余弦流光扫描带参数（与 `sai` / Codex 保持一致） ──

const SHIMMER_PADDING: usize = 10;
const SHIMMER_SWEEP_SECONDS: f32 = 2.0;
const SHIMMER_BAND_HALF_WIDTH: f32 = 5.0;

/// 按真实运行时长换算统一帧号，避免终端轮询抖动影响动画速度。
pub fn frame_at(elapsed: Duration) -> usize {
    let interval = ACTIVITY_FRAME_INTERVAL.as_micros().max(1);
    (elapsed.as_micros() / interval) as usize
}

/// 按帧号选择固定 5 列的状态伴随表情（不规则眨眼优先于神情轮换）。
pub fn face_at(frame: usize) -> &'static str {
    let (offset, span, _) = schedule_segment(frame, BLINK_SALT, BLINK_GAP_MIN, BLINK_GAP_SPREAD);
    if offset + BLINK_FRAMES >= span {
        return BLINK_FACE;
    }
    let (_, _, hash) = schedule_segment(frame, MOOD_SALT, MOOD_MIN, MOOD_SPREAD);
    let choice = (hash / 97) as usize % (GLANCE_FACES.len() * 2);
    GLANCE_FACES.get(choice).copied().unwrap_or(DEFAULT_FACE)
}

fn schedule_segment(frame: usize, salt: u32, min: usize, spread: usize) -> (usize, usize, u32) {
    let segment = |index: u32| {
        let hash = mix(index ^ salt);
        (min + hash as usize % spread, hash)
    };
    let cycle: usize = (0..SCHEDULE_SEGMENTS).map(|index| segment(index).0).sum();
    let mut offset = frame % cycle;
    for index in 0..SCHEDULE_SEGMENTS {
        let (span, hash) = segment(index);
        if offset < span {
            return (offset, span, hash);
        }
        offset -= span;
    }
    (0, min, 0)
}

fn mix(value: u32) -> u32 {
    let value = ((value >> 16) ^ value).wrapping_mul(0x045d_9f3b);
    let value = ((value >> 16) ^ value).wrapping_mul(0x045d_9f3b);
    (value >> 16) ^ value
}

fn blend_rgb(from: (u8, u8, u8), to: (u8, u8, u8), ratio: f32) -> Color {
    let ratio = ratio.clamp(0.0, 1.0);
    let mix_ch = |a: u8, b: u8| (b as f32 * ratio + a as f32 * (1.0 - ratio)) as u8;
    Color::Rgb(
        mix_ch(from.0, to.0),
        mix_ch(from.1, to.1),
        mix_ch(from.2, to.2),
    )
}

/// 生成 `sai` 风格的流光文字 spans（2 秒周期余弦扫描带，前景向背景混色）。
pub fn shimmer_spans(text: &str, frame: usize) -> Vec<Span<'static>> {
    let p = theme::palette();
    shimmer_spans_with_colors(text, frame, p.text, p.background, p.on_accent)
}

fn shimmer_spans_with_colors(
    text: &str,
    frame: usize,
    fg_color: Color,
    bg_color: Color,
    fallback_bg: Color,
) -> Vec<Span<'static>> {
    if text.is_empty() {
        return Vec::new();
    }
    let elapsed_secs = frame as f32 * ACTIVITY_FRAME_INTERVAL.as_secs_f32();
    let period = text.chars().count() + SHIMMER_PADDING * 2;
    let position =
        ((elapsed_secs % SHIMMER_SWEEP_SECONDS) / SHIMMER_SWEEP_SECONDS * period as f32) as usize;

    let fg_rgb = match fg_color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    };
    // 透明主题（background = Reset）时回退到 on_accent 作为暗调基底，确保依然具备丝滑真彩扫光
    let bg_rgb = match bg_color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => match fallback_bg {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            _ => None,
        },
    };

    text.chars()
        .enumerate()
        .map(|(index, ch)| {
            let distance = (index + SHIMMER_PADDING).abs_diff(position) as f32;
            let intensity = if distance <= SHIMMER_BAND_HALF_WIDTH {
                0.5 * (1.0 + (std::f32::consts::PI * (distance / SHIMMER_BAND_HALF_WIDTH)).cos())
            } else {
                0.0
            };
            let style = match (fg_rgb, bg_rgb) {
                (Some(fg), Some(bg)) => Style::new()
                    .fg(blend_rgb(fg, bg, intensity.clamp(0.0, 1.0) * 0.9))
                    .add_modifier(Modifier::BOLD),
                _ => {
                    if intensity < 0.2 {
                        Style::new().fg(fg_color).add_modifier(Modifier::DIM)
                    } else if intensity >= 0.6 {
                        Style::new().fg(fg_color).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(fg_color)
                    }
                }
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

/// 组合完整的底部动态状态行：`左侧眨眼表情` + `流光状态文字` + `弱化耗时详情`。
pub fn render_status_line(label: &str, detail: &str, frame: usize) -> Line<'static> {
    let mut spans = Vec::with_capacity(label.chars().count() + 4);
    // 表情使用正常前景色且不加粗，动感由眨眼与神情变化自然呈现
    spans.push(Span::styled(face_at(frame), theme::text()));
    if !label.is_empty() {
        spans.push(Span::raw(" "));
        spans.extend(shimmer_spans(label, frame));
    }
    if !detail.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(detail.to_string(), theme::dim()));
    }
    Line::from(spans)
}

/// 根据当前会话时间线的最新活跃节点推导工作状态文案（借鉴 `sai` 的 `WorkStatus`）。
pub fn infer_work_status(transcript: &Transcript) -> &'static str {
    let Some(last) = transcript.entries.last() else {
        return "等待响应";
    };
    match &last.kind {
        EntryKind::Thought { elapsed: None, .. } => "思考中",
        EntryKind::Tool {
            status, kind, name, ..
        } if !matches!(
            status.as_str(),
            "completed" | "failed" | "error" | "cancelled"
        ) =>
        {
            status_for_tool(kind, name)
        }
        EntryKind::Assistant { done_line: None } if !last.text.trim().is_empty() => "正在生成",
        _ => "等待响应",
    }
}

fn status_for_tool(kind: &str, name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("compact") || lower.contains("compress") {
        return "压缩上下文";
    }
    match kind {
        "edit" => "等待写入",
        "read" => "正在读取",
        "execute" => "正在执行",
        "search" => "正在搜索",
        "fetch" => "正在获取",
        _ => {
            if lower.contains("write")
                || lower.contains("edit")
                || lower.contains("replace")
                || lower.contains("patch")
            {
                "等待写入"
            } else if lower.contains("read") || lower.contains("view") {
                "正在读取"
            } else {
                "等待运行"
            }
        }
    }
}

/// 格式化底部状态行耗时：采用整数秒（`0s` / `12s` / `1m 05s`），避免小数秒高频跳动。
pub fn format_elapsed(elapsed: Duration) -> String {
    let total_secs = elapsed.as_secs();
    if total_secs < 60 {
        format!("{total_secs}s")
    } else {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{mins}m {secs:02}s")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use unicode_width::UnicodeWidthStr;

    use super::*;
    use crate::transcript::Transcript;

    #[test]
    fn companion_faces_have_fixed_width_and_blink_irregularly() {
        let mut seen = HashSet::new();
        let mut saw_blink = false;
        for frame in 0..800 {
            let face = face_at(frame);
            assert_eq!(face.width(), 5, "face width must stay 5 cols: {face:?}");
            if face == BLINK_FACE {
                saw_blink = true;
            }
            seen.insert(face);
        }
        assert!(saw_blink, "companion must blink within 800 frames");
        assert!(seen.contains(DEFAULT_FACE), "companion must show default face");
        assert!(
            seen.len() >= 3,
            "companion should cycle through multiple expressions: {seen:?}"
        );
    }

    #[test]
    fn shimmer_sweeps_across_frames_for_dark_light_and_transparent_themes() {
        let cases = [
            (
                "dark",
                Color::Rgb(0xc0, 0xca, 0xf5),
                Color::Rgb(0x1a, 0x1b, 0x26),
                Color::Rgb(0x1a, 0x1b, 0x26),
            ),
            (
                "light",
                Color::Rgb(0x37, 0x60, 0xbf),
                Color::Rgb(0xe1, 0xe2, 0xe7),
                Color::Rgb(0xe1, 0xe2, 0xe7),
            ),
            (
                "transparent",
                Color::Rgb(0xc0, 0xca, 0xf5),
                Color::Reset,
                Color::Rgb(0x1a, 0x1b, 0x26),
            ),
        ];
        for (name, fg, bg, fallback_bg) in cases {
            let f0 = shimmer_spans_with_colors("正在生成", 0, fg, bg, fallback_bg);
            let f28 = shimmer_spans_with_colors("正在生成", 28, fg, bg, fallback_bg);
            assert_eq!(f0.len(), 4);
            assert_eq!(f28.len(), 4);
            let colors0: Vec<_> = f0.iter().map(|s| s.style.fg).collect();
            let colors28: Vec<_> = f28.iter().map(|s| s.style.fg).collect();
            assert_ne!(
                colors0, colors28,
                "shimmer colors should animate across frames for {name}"
            );
        }
    }

    #[test]
    fn work_status_tracks_transcript_activity() {
        let mut tr = Transcript::new();
        assert_eq!(infer_work_status(&tr), "等待响应");

        tr.append_thought("analyzing...");
        assert_eq!(infer_work_status(&tr), "思考中");

        tr.upsert_tool(
            "c1".into(),
            Some("read_file".into()),
            Some("src/main.rs".into()),
            Some("read".into()),
            Some("in_progress".into()),
            None,
            None,
            Vec::new(),
        );
        assert_eq!(infer_work_status(&tr), "正在读取");

        tr.upsert_tool(
            "c1".into(),
            None,
            None,
            None,
            Some("completed".into()),
            None,
            Some("ok".into()),
            Vec::new(),
        );
        assert_eq!(infer_work_status(&tr), "等待响应");

        tr.upsert_tool(
            "c2".into(),
            Some("edit_file".into()),
            Some("src/ui/mod.rs".into()),
            Some("edit".into()),
            Some("in_progress".into()),
            None,
            None,
            Vec::new(),
        );
        assert_eq!(infer_work_status(&tr), "等待写入");

        tr.append_assistant("已更新底部动效。");
        assert_eq!(infer_work_status(&tr), "正在生成");
    }

    #[test]
    fn elapsed_uses_calm_integer_seconds() {
        assert_eq!(format_elapsed(Duration::from_millis(999)), "0s");
        assert_eq!(format_elapsed(Duration::from_millis(1500)), "1s");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1m 05s");
    }
}
