//! 时间戳解析、相对/紧凑时间格式化与跨平台路径缩写工具。

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::linebreak;

/// 解析会话时间戳（支持 RFC3339 / ISO8601 / Unix 秒或毫秒 / `YYYY-MM-DD HH:MM:SS`），返回 Unix 秒。
pub fn parse_timestamp_secs(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    if let Ok(n) = s.parse::<i64>() {
        // 13 位及以上按毫秒处理，10 位按秒处理
        return Some(if n.abs() >= 100_000_000_000 {
            n / 1000
        } else {
            n
        });
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt)
            && let Some(dt) = Local.from_local_datetime(&naive).earliest() {
                return Some(dt.timestamp());
            }
    }
    None
}

/// 将会话更新时间格式化为人类友好的相对/短时间（如 `刚刚`、`5分钟前`、`今天 14:20`、`昨天 09:15`、`11-20`）。
pub fn format_relative_time(raw: &str) -> String {
    let Some(ts) = parse_timestamp_secs(raw) else {
        let trimmed = raw.trim();
        return if trimmed.is_empty() {
            "未知时间".to_string()
        } else {
            linebreak::clip(trimmed, 16)
        };
    };
    let now = Local::now();
    let Some(dt) = Local.timestamp_opt(ts, 0).single() else {
        return "未知时间".to_string();
    };
    let diff = now.timestamp().saturating_sub(ts);
    if diff < 60 {
        "刚刚".to_string()
    } else if diff < 3600 {
        format!("{}分钟前", diff / 60)
    } else if diff < 6 * 3600 {
        format!("{}小时前", diff / 3600)
    } else if dt.date_naive() == now.date_naive() {
        dt.format("今天 %H:%M").to_string()
    } else if now.date_naive().signed_duration_since(dt.date_naive()).num_days() == 1 {
        dt.format("昨天 %H:%M").to_string()
    } else if dt.year() == now.year() {
        dt.format("%m-%d %H:%M").to_string()
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

/// 紧凑版相对时间（专供右侧边栏窄列展示，宽度控制在 4~6 列：如 `刚刚`、`3分前`、`2时前`、`昨天`、`5天前`、`11-20`）。
pub fn format_compact_time(raw: &str) -> String {
    let Some(ts) = parse_timestamp_secs(raw) else {
        return String::new();
    };
    let now = Local::now();
    let Some(dt) = Local.timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let diff = now.timestamp().saturating_sub(ts);
    if diff < 60 {
        "刚刚".to_string()
    } else if diff < 3600 {
        format!("{}分前", diff / 60)
    } else if diff < 24 * 3600 && dt.date_naive() == now.date_naive() {
        format!("{}时前", diff / 3600)
    } else {
        let days = now.date_naive().signed_duration_since(dt.date_naive()).num_days();
        if days <= 0 {
            format!("{}时前", (diff / 3600).max(1))
        } else if days == 1 {
            "昨天".to_string()
        } else if days < 30 {
            format!("{days}天前")
        } else if dt.year() == now.year() {
            dt.format("%m-%d").to_string()
        } else {
            dt.format("%y-%m").to_string()
        }
    }
}

/// 跨平台规范化目录路径比较（Windows 忽略盘符/目录大小写与正反斜杠差异）。
pub fn same_directory(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        let replaced = p.trim().replace('\\', "/");
        let trimmed = replaced.trim_end_matches('/');
        #[cfg(windows)]
        {
            trimmed.to_ascii_lowercase()
        }
        #[cfg(not(windows))]
        {
            trimmed.to_string()
        }
    };
    let na = norm(a);
    let nb = norm(b);
    !na.is_empty() && na == nb
}

/// 精简路径：用户主目录折叠为 `~`，超宽时保留末尾项目目录名、左侧省略 `…`。
pub fn shorten_path_left(path: &str, max_width: u16) -> String {
    let room = usize::from(max_width);
    if room == 0 {
        return String::new();
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let compact = if !home.is_empty() {
        let home_trim = home.trim_end_matches(['\\', '/']);
        if let Some(prefix) = path.get(..home_trim.len()) {
            let rest = &path[home_trim.len()..];
            let at_sep = rest.is_empty() || rest.starts_with(['\\', '/']);
            let matches_home = if cfg!(windows) {
                prefix.eq_ignore_ascii_case(home_trim)
            } else {
                prefix == home_trim
            };
            if matches_home && at_sep {
                format!("~{rest}")
            } else {
                path.to_string()
            }
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };

    if compact.width() <= room {
        return compact;
    }
    if room <= 1 {
        return "…".to_string();
    }
    let target = room - 1;
    let graphemes: Vec<&str> = compact.graphemes(true).collect();
    let mut used = 0usize;
    let mut start = graphemes.len();
    while start > 0 {
        let w = graphemes[start - 1].width();
        if used + w > target {
            break;
        }
        used += w;
        start -= 1;
    }
    format!("…{}", graphemes[start..].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_and_epoch_timestamps_and_matches_dirs() {
        assert!(parse_timestamp_secs("2026-11-24T08:30:00Z").is_some());
        assert_eq!(parse_timestamp_secs("1700000000"), Some(1700000000));
        assert_eq!(parse_timestamp_secs("1700000000123"), Some(1700000000));
        assert!(same_directory(r"D:\Code\agy-tui\", "D:/Code/agy-tui"));
        let short = shorten_path_left(r"D:\very\long\workspace\path\my-project", 18);
        assert!(short.ends_with("my-project"), "should keep tail: {short}");
        assert!(short.starts_with('…'));
        // 中文目录在 byte 11 恰好落在多字节 UTF-8 字符内部，绝不能 panic
        let cjk = shorten_path_left(r"D:\work\11月\11.23-DEPLOYC-18352-无限", 20);
        assert!(cjk.ends_with("无限"), "should handle CJK path safely: {cjk}");
    }
}
