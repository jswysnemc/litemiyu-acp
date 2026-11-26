//! 客户端自身的配置与缓存目录解析（与具体 ACP 后端完全解耦，不写入 `~/.gemini`）。
//! - Unix 遵循 XDG Base Directory 规范（`$XDG_CONFIG_HOME` / `$XDG_CACHE_HOME`）
//! - Windows 使用 `%APPDATA%` / `%LOCALAPPDATA%`
//! - 保留对旧版 `~/.gemini/litemiyu-acp.json` 的只读兼容回退

use std::ffi::OsString;
use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "litemiyu-acp";

/// 返回用户偏好配置文件路径。
///
/// 返回:
/// - Unix: `$XDG_CONFIG_HOME/litemiyu-acp/prefs.json` 或 `~/.config/litemiyu-acp/prefs.json`
/// - Windows: `%APPDATA%\litemiyu-acp\prefs.json`（缺失时回退 `%USERPROFILE%\.config\litemiyu-acp\prefs.json`）
pub fn prefs_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("prefs.json"))
}

/// 返回旧版配置文件路径（仅用于首次升级时的兼容读取，不再写入）。
///
/// 返回:
/// - 旧版 `~/.gemini/litemiyu-acp.json` 路径
pub fn legacy_prefs_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".gemini").join("litemiyu-acp.json"))
}

/// 返回剪贴板截图落盘缓存目录。
///
/// 返回:
/// - Unix: `$XDG_CACHE_HOME/litemiyu-acp/clips` 或 `~/.cache/litemiyu-acp/clips`
/// - Windows: `%LOCALAPPDATA%\litemiyu-acp\clips`（回退系统临时目录）
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn clip_cache_dir() -> Option<PathBuf> {
    cache_dir()
        .map(|d| d.join("clips"))
        .or_else(|| Some(std::env::temp_dir().join(APP_DIR_NAME).join("clips")))
}

/// 返回 agy 本地会话脑目录（仅用于只读观察子代理与父会话 `transcript.jsonl`，绝不写入）。
///
/// 返回:
/// - 默认路径 `~/.gemini/antigravity-cli/brain`
pub fn agy_brain_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".gemini").join("antigravity-cli").join("brain"))
}

/// 校验会话或子代理 ID 是否安全（禁止路径分隔符、相对跳转与控制字符）。
///
/// 参数:
/// - `id`: 待校验的会话标识字符串
///
/// 返回:
/// - 安全合法时返回 `true`，否则返回 `false`
pub fn is_safe_session_id(id: &str) -> bool {
    let trimmed = id.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed == "." || trimmed == ".." {
        return false;
    }
    trimmed == id && trimmed.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 在指定 `brain_root` 下拼接会话对应的 `transcript.jsonl` 只读路径。
///
/// 参数:
/// - `brain_root`: 会话脑根目录
/// - `session_id`: 会话或子代理的 `conversationId`
///
/// 返回:
/// - 校验通过时返回 `<brain_root>/<session_id>/.system_generated/logs/transcript.jsonl`
pub fn agy_transcript_path(brain_root: &Path, session_id: &str) -> Option<PathBuf> {
    if !is_safe_session_id(session_id) {
        return None;
    }
    Some(
        brain_root
            .join(session_id.trim())
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl"),
    )
}

/// 返回默认 `agy` 脑目录下指定子代理的 `transcript.jsonl` 路径。
///
/// 参数:
/// - `conversation_id`: 子代理会话 ID
///
/// 返回:
/// - 路径合法且主目录可解析时返回 `Some(PathBuf)`
#[allow(dead_code)]
pub fn agy_subagent_transcript_path(conversation_id: &str) -> Option<PathBuf> {
    agy_brain_dir().and_then(|root| agy_transcript_path(&root, conversation_id))
}

/// 将 `file://` URI 解析为本地文件路径（支持百分号解码与 Windows 盘符标准化）。
///
/// 参数:
/// - `uri`: 如 `file:///home/user/.gemini/.../transcript.jsonl` 或 `file:///C:/Users/...`
///
/// 返回:
/// - 合法 `file://` URI 返回对应 `PathBuf`，否则返回 `None`
pub fn parse_file_uri(uri: &str) -> Option<PathBuf> {
    parse_file_uri_for_platform(uri, cfg!(windows))
}

/// 按目标平台规则解析 `file://` URI（抽离平台标志便于跨平台单元测试）。
///
/// 参数:
/// - `uri`: 原始 URI 字符串
/// - `is_windows`: 是否按 Windows 盘符与路径分隔符规则转换
///
/// 返回:
/// - 解析成功返回 `Some(PathBuf)`，失败返回 `None`
fn parse_file_uri_for_platform(uri: &str, is_windows: bool) -> Option<PathBuf> {
    let trimmed = uri.trim();
    // 1. 校验协议头并剥离 `file://` 与可选的 `localhost`
    let prefix = trimmed.get(..7)?;
    if !prefix.eq_ignore_ascii_case("file://") {
        return None;
    }
    let mut rest = &trimmed[7..];
    if let Some(after_host) = rest
        .get(..9)
        .filter(|h| h.eq_ignore_ascii_case("localhost"))
        .and_then(|_| rest.get(9..))
        && after_host.starts_with('/')
    {
        rest = after_host;
    }
    if rest.is_empty() {
        return None;
    }
    // 2. 百分号解码 UTF-8 字节序列
    let decoded = percent_decode(rest)?;
    if decoded.is_empty() {
        return None;
    }
    // 3. Windows 平台剥离 `/C:/` 前导斜杠并统一反斜杠
    if is_windows {
        let bytes = decoded.as_bytes();
        let norm = if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':'
        {
            decoded[1..].replace('/', "\\")
        } else {
            decoded.replace('/', "\\")
        };
        return Some(PathBuf::from(norm));
    }
    Some(PathBuf::from(decoded))
}

/// 对 URI 路径中的 `%XX` 百分号编码进行 UTF-8 字节解码。
///
/// 参数:
/// - `input`: 编码后的路径字符串
///
/// 返回:
/// - 有效 UTF-8 字符串返回 `Some(String)`，非法字节返回 `None`
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(
        cfg!(windows),
        std::env::var_os("APPDATA"),
        std::env::var_os("XDG_CONFIG_HOME"),
        home_dir(),
    )
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn cache_dir() -> Option<PathBuf> {
    resolve_cache_dir(
        cfg!(windows),
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("XDG_CACHE_HOME"),
        home_dir(),
    )
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn resolve_config_dir(
    is_windows: bool,
    appdata: Option<OsString>,
    xdg_config: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if is_windows {
        if let Some(dir) = appdata.filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(dir).join(APP_DIR_NAME));
        }
    } else if let Some(dir) = xdg_config.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join(APP_DIR_NAME));
    }
    home.map(|h| h.join(".config").join(APP_DIR_NAME))
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn resolve_cache_dir(
    is_windows: bool,
    local_appdata: Option<OsString>,
    xdg_cache: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if is_windows {
        if let Some(dir) = local_appdata.filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(dir).join(APP_DIR_NAME));
        }
    } else if let Some(dir) = xdg_cache.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join(APP_DIR_NAME));
    }
    home.map(|h| h.join(".cache").join(APP_DIR_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_xdg_and_default_unix_paths_without_gemini_dir() {
        let home = Some(PathBuf::from("/home/tester"));
        let xdg_cfg = resolve_config_dir(false, None, Some(OsString::from("/etc/xdg-user")), home.clone());
        assert_eq!(xdg_cfg, Some(PathBuf::from("/etc/xdg-user/litemiyu-acp")));

        let def_cfg = resolve_config_dir(false, None, None, home.clone());
        assert_eq!(def_cfg, Some(PathBuf::from("/home/tester/.config/litemiyu-acp")));

        let xdg_cache = resolve_cache_dir(false, None, Some(OsString::from("/tmp/cache")), home.clone());
        assert_eq!(xdg_cache, Some(PathBuf::from("/tmp/cache/litemiyu-acp")));

        let def_cache = resolve_cache_dir(false, None, None, home);
        assert_eq!(def_cache, Some(PathBuf::from("/home/tester/.cache/litemiyu-acp")));
    }

    #[test]
    fn resolves_windows_appdata_paths_without_gemini_dir() {
        let home = Some(PathBuf::from(r"C:\Users\tester"));
        let cfg = resolve_config_dir(
            true,
            Some(OsString::from(r"C:\Users\tester\AppData\Roaming")),
            None,
            home.clone(),
        );
        assert_eq!(
            cfg,
            Some(PathBuf::from(r"C:\Users\tester\AppData\Roaming").join("litemiyu-acp"))
        );

        let cache = resolve_cache_dir(
            true,
            Some(OsString::from(r"C:\Users\tester\AppData\Local")),
            None,
            home,
        );
        assert_eq!(
            cache,
            Some(PathBuf::from(r"C:\Users\tester\AppData\Local").join("litemiyu-acp"))
        );
    }

    #[test]
    fn validates_agy_transcript_paths_and_parses_file_uris() {
        let root = PathBuf::from("/tmp/brain");
        let valid = agy_transcript_path(&root, "28348559-16f4-4b8e-847a-13f32e7bac97");
        assert_eq!(
            valid,
            Some(PathBuf::from(
                "/tmp/brain/28348559-16f4-4b8e-847a-13f32e7bac97/.system_generated/logs/transcript.jsonl"
            ))
        );
        assert!(agy_transcript_path(&root, "../etc").is_none());
        assert!(agy_transcript_path(&root, "foo/bar").is_none());
        assert!(agy_transcript_path(&root, "foo\\bar").is_none());
        assert!(agy_transcript_path(&root, "").is_none());

        assert_eq!(
            parse_file_uri_for_platform("file:///home/u/my%20dir/%E4%B8%AD.jsonl", false),
            Some(PathBuf::from("/home/u/my dir/中.jsonl"))
        );
        assert_eq!(
            parse_file_uri_for_platform("file://localhost/tmp/a.jsonl", false),
            Some(PathBuf::from("/tmp/a.jsonl"))
        );
        assert_eq!(
            parse_file_uri_for_platform("file:///C:/Users/demo/transcript.jsonl", true),
            Some(PathBuf::from(r"C:\Users\demo\transcript.jsonl"))
        );
        assert!(parse_file_uri_for_platform("https://example.com/a", false).is_none());
    }
}
