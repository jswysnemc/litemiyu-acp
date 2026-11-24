//! 客户端自身的配置与缓存目录解析（与具体 ACP 后端完全解耦，不写入 `~/.gemini`）。
//! - Unix 遵循 XDG Base Directory 规范（`$XDG_CONFIG_HOME` / `$XDG_CACHE_HOME`）
//! - Windows 使用 `%APPDATA%` / `%LOCALAPPDATA%`
//! - 保留对旧版 `~/.gemini/litemiyu-acp.json` 的只读兼容回退

use std::ffi::OsString;
use std::path::PathBuf;

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
}
