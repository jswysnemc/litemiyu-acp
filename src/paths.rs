//! 客户端自身的配置与缓存目录解析（与具体 ACP 后端完全解耦，不写入 `~/.gemini`）。
//! - Unix 遵循 XDG Base Directory 规范（`$XDG_CONFIG_HOME` / `$XDG_CACHE_HOME`）
//! - Windows 使用 `%APPDATA%` / `%LOCALAPPDATA%`
//! - 保留对旧版 `litemiyu-acp` 配置路径及 `~/.gemini/litemiyu-acp.json` 的只读兼容回退

use std::ffi::OsString;
use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "agy-tui";
const LEGACY_APP_DIR_NAME: &str = "litemiyu-acp";

/// 返回用户偏好配置文件路径。
///
/// 返回:
/// - Unix: `$XDG_CONFIG_HOME/agy-tui/prefs.json` 或 `~/.config/agy-tui/prefs.json`
/// - Windows: `%APPDATA%\agy-tui\prefs.json`（缺失时回退 `%USERPROFILE%\.config\agy-tui\prefs.json`）
pub fn prefs_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("prefs.json"))
}

/// 返回用户自定义主题目录路径。
///
/// 返回:
/// - Unix: `$XDG_CONFIG_HOME/agy-tui/themes` 或 `~/.config/agy-tui/themes`
/// - Windows: `%APPDATA%\agy-tui\themes`（缺失时回退 `%USERPROFILE%\.config\agy-tui\themes`）
pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

/// 按优先级返回旧版配置文件候选路径（仅用于首次升级时的只读兼容迁移，不再写入）。
///
/// 返回:
/// - 依次包含旧版独立目录 `litemiyu-acp/prefs.json` 与早期 `~/.gemini/litemiyu-acp.json`
pub fn legacy_prefs_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // 1. 优先检查更名之前的独立配置目录 litemiyu-acp/prefs.json
    if let Some(dir) = config_dir_for(LEGACY_APP_DIR_NAME) {
        paths.push(dir.join("prefs.json"));
    }
    // 2. 回退检查最早期的 ~/.gemini/litemiyu-acp.json
    if let Some(home) = home_dir() {
        paths.push(home.join(".gemini").join("litemiyu-acp.json"));
    }
    paths
}

/// 返回剪贴板截图落盘缓存目录。
///
/// 返回:
/// - Unix: `$XDG_CACHE_HOME/agy-tui/clips` 或 `~/.cache/agy-tui/clips`
/// - Windows: `%LOCALAPPDATA%\agy-tui\clips`（回退系统临时目录）
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn clip_cache_dir() -> Option<PathBuf> {
    cache_dir()
        .map(|d| d.join("clips"))
        .or_else(|| Some(std::env::temp_dir().join(APP_DIR_NAME).join("clips")))
}

/// 折叠用户主目录前缀为 `~`（仅匹配完整路径边界，避免误替换相似前缀）。
///
/// 参数:
/// - `path`: 原始目录字符串
///
/// 返回:
/// - 若以当前用户主目录开头，返回 `~` 缩写路径，否则原样返回
pub fn collapse_home_dir(path: &str) -> String {
    let Some(home) = home_dir() else {
        return path.to_string();
    };
    let home_str = home.to_string_lossy();
    let trimmed_home = home_str.trim_end_matches(['/', '\\']);
    if trimmed_home.is_empty() {
        return path.to_string();
    }
    if path == trimmed_home {
        return "~".to_string();
    }
    if let Some(rest) = path.strip_prefix(trimmed_home)
        && (rest.starts_with('/') || rest.starts_with('\\'))
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// 纯文件读取方式探测给定目录所在的 Git 分支名（零子进程开销，支持普通仓库与 worktree/submodule）。
///
/// 参数:
/// - `dir`: 工作区目录路径
///
/// 返回:
/// - 处于命名分支时返回分支名（如 `main`）
/// - 处于游离 HEAD 时返回 `@<前7位哈希>`
/// - 非 Git 仓库或读取失败时返回 `None`
pub fn git_branch_in(dir: &Path) -> Option<String> {
    // 1. 沿祖先目录向上查找 `.git`（目录或 worktree/submodule 指针文件）
    let mut cur = Some(dir);
    let git_dir = loop {
        let candidate = cur?.join(".git");
        if candidate.is_dir() {
            break candidate;
        }
        if candidate.is_file() {
            let content = std::fs::read_to_string(&candidate).ok()?;
            let first = content.lines().next()?.trim();
            let raw_target = first.strip_prefix("gitdir:")?.trim();
            if raw_target.is_empty() {
                return None;
            }
            let target = PathBuf::from(raw_target);
            break if target.is_absolute() {
                target
            } else {
                cur?.join(target)
            };
        }
        cur = cur?.parent();
    };

    // 2. 读取 HEAD 文件解析当前分支或游离提交短哈希
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let line = head.lines().next()?.trim();
    if let Some(ref_path) = line.strip_prefix("ref:") {
        let trimmed = ref_path.trim();
        let branch = trimmed
            .strip_prefix("refs/heads/")
            .or_else(|| trimmed.strip_prefix("refs/tags/"))
            .or_else(|| trimmed.rsplit('/').next())
            .unwrap_or(trimmed);
        return (!branch.is_empty()).then(|| branch.to_string());
    }
    if line.len() >= 7 && line.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(format!("@{}", &line[..7]));
    }
    None
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
    config_dir_for(APP_DIR_NAME)
}

/// 按给定应用目录名解析当前平台的配置根目录。
///
/// 参数:
/// - `app_dir_name`: 应用配置子目录名
///
/// 返回:
/// - 平台对应的配置目录路径
fn config_dir_for(app_dir_name: &str) -> Option<PathBuf> {
    resolve_config_dir_for(
        cfg!(windows),
        std::env::var_os("APPDATA"),
        std::env::var_os("XDG_CONFIG_HOME"),
        home_dir(),
        app_dir_name,
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

#[cfg(test)]
fn resolve_config_dir(
    is_windows: bool,
    appdata: Option<OsString>,
    xdg_config: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    resolve_config_dir_for(is_windows, appdata, xdg_config, home, APP_DIR_NAME)
}

fn resolve_config_dir_for(
    is_windows: bool,
    appdata: Option<OsString>,
    xdg_config: Option<OsString>,
    home: Option<PathBuf>,
    app_dir_name: &str,
) -> Option<PathBuf> {
    if is_windows {
        if let Some(dir) = appdata.filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(dir).join(app_dir_name));
        }
    } else if let Some(dir) = xdg_config.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join(app_dir_name));
    }
    home.map(|h| h.join(".config").join(app_dir_name))
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
        assert_eq!(xdg_cfg, Some(PathBuf::from("/etc/xdg-user/agy-tui")));

        let def_cfg = resolve_config_dir(false, None, None, home.clone());
        assert_eq!(def_cfg, Some(PathBuf::from("/home/tester/.config/agy-tui")));

        let legacy_cfg = resolve_config_dir_for(
            false,
            None,
            Some(OsString::from("/etc/xdg-user")),
            home.clone(),
            LEGACY_APP_DIR_NAME,
        );
        assert_eq!(legacy_cfg, Some(PathBuf::from("/etc/xdg-user/litemiyu-acp")));

        let xdg_cache = resolve_cache_dir(false, None, Some(OsString::from("/tmp/cache")), home.clone());
        assert_eq!(xdg_cache, Some(PathBuf::from("/tmp/cache/agy-tui")));

        let def_cache = resolve_cache_dir(false, None, None, home);
        assert_eq!(def_cache, Some(PathBuf::from("/home/tester/.cache/agy-tui")));
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
            Some(PathBuf::from(r"C:\Users\tester\AppData\Roaming").join("agy-tui"))
        );

        let cache = resolve_cache_dir(
            true,
            Some(OsString::from(r"C:\Users\tester\AppData\Local")),
            None,
            home,
        );
        assert_eq!(
            cache,
            Some(PathBuf::from(r"C:\Users\tester\AppData\Local").join("agy-tui"))
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

    #[test]
    fn detects_git_branch_detached_head_and_worktree_gitdir() {
        let base = std::env::temp_dir().join(format!("agy_tui_git_branch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // 1. 普通仓库命名分支（子目录内向上查找）
        let repo = base.join("repo");
        let sub = repo.join("src/ui");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feat/theme-colors\n").unwrap();
        assert_eq!(git_branch_in(&sub).as_deref(), Some("feat/theme-colors"));

        // 2. 游离 HEAD 返回 @ 前 7 位短哈希
        std::fs::write(
            repo.join(".git/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert_eq!(git_branch_in(&repo).as_deref(), Some("@0123456"));

        // 3. worktree 形式（.git 文件指向独立 gitdir）
        let wt = base.join("wt");
        let wt_git = base.join("wt-gitdir");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        std::fs::write(wt_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(git_branch_in(&wt).as_deref(), Some("main"));

        // 4. 非 Git 目录返回 None
        let plain = base.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(git_branch_in(&plain), None);

        let _ = std::fs::remove_dir_all(&base);
    }
}
