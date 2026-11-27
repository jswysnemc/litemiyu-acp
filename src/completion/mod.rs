//! 输入框三触发符补全引擎：
//! - `/`：客户端本地命令 + ACP 后端非 Skill 斜杠命令
//! - `#`：跨 Agent 标准 Skills 目录（`SKILL.md`）与后端映射 Skills 模糊搜索
//! - `@`：当前工作区文件轻量索引与路径模糊搜索

mod scan;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::acp::SlashCommandInfo;

const FILES_CACHE_TTL: Duration = Duration::from_secs(10);
const SKILLS_CACHE_TTL: Duration = Duration::from_secs(30);
const MAX_RESULTS: usize = 40;

use scan::{discover_skills, scan_workspace_files, score_file_path, score_skill};

/// 输入补全触发模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// `/` 触发斜杠命令
    Command,
    /// `#` 触发 Skills 搜索
    Skill,
    /// `@` 触发工作区文件搜索
    File,
}

/// 当前光标位置解析出的补全触发上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionTrigger {
    pub kind: CompletionKind,
    pub query: String,
    /// 替换起始字节偏移（相对输入框全文起点）
    pub replace_start: usize,
}

/// 补全悬浮列表的单条候选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// 列表左侧主标题（如 `/model`、`#code-review`、`src/main.rs`）
    pub label: String,
    /// 选中后替换进输入框的文本（通常自带尾随空格）
    pub insert_text: String,
    /// 列表右侧辅助描述（命令说明 / Skill 描述 / 相对目录）
    pub description: String,
    /// 是否使用主强调色渲染左侧标签（用于区分本地命令与远端命令）
    pub accent_primary: bool,
    /// 原始命令名（仅 `Command` 模式用于 Enter 直达执行）
    pub command_name: Option<String>,
}

/// 供 UI 渲染与按键交互使用的完整补全菜单快照。
#[derive(Debug, Clone)]
pub struct CompletionMenu {
    pub kind: CompletionKind,
    pub replace_start: usize,
    pub items: Vec<CompletionItem>,
}

/// 本地发现的单个 Skill 元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// 工作区文件与 Skills 索引缓存，避免按键输入时反复触盘。
#[derive(Debug, Default)]
pub struct CompletionCache {
    cwd: String,
    files: Vec<String>,
    files_at: Option<Instant>,
    skills: Vec<SkillEntry>,
    skills_at: Option<Instant>,
}

impl CompletionCache {
    /// 切换工作区或重置会话时清空缓存，确保下次触发读取最新目录内容。
    pub fn invalidate(&mut self) {
        self.files.clear();
        self.files_at = None;
        self.skills.clear();
        self.skills_at = None;
    }

    /// 返回当前工作区对应的 Skill 列表（带 30 秒 TTL 缓存）。
    ///
    /// 参数:
    /// - `cwd`: 当前工作区根目录路径
    ///
    /// 返回:
    /// - 缓存的 `SkillEntry` 切片
    pub fn skills_for(&mut self, cwd: &str) -> &[SkillEntry] {
        if self.cwd != cwd {
            self.cwd = cwd.to_string();
            self.invalidate();
        }
        let stale = self
            .skills_at
            .is_none_or(|at| at.elapsed() >= SKILLS_CACHE_TTL);
        if stale {
            self.skills = discover_skills(cwd);
            self.skills_at = Some(Instant::now());
        }
        &self.skills
    }

    /// 返回当前工作区文件相对路径列表（带 10 秒 TTL 缓存）。
    ///
    /// 参数:
    /// - `cwd`: 当前工作区根目录路径
    ///
    /// 返回:
    /// - 缓存的相对路径切片
    pub fn files_for(&mut self, cwd: &str) -> &[String] {
        if self.cwd != cwd {
            self.cwd = cwd.to_string();
            self.invalidate();
        }
        let stale = self
            .files_at
            .is_none_or(|at| at.elapsed() >= FILES_CACHE_TTL);
        if stale {
            self.files = scan_workspace_files(cwd);
            self.files_at = Some(Instant::now());
        }
        &self.files
    }

    /// 根据当前输入框内容与光标位置构建补全菜单（若未触发或无匹配则返回 `None`）。
    ///
    /// 参数:
    /// - `cwd`: 当前工作区目录
    /// - `full_text`: 输入框完整文本
    /// - `text_before_cursor`: 光标前的输入切片
    /// - `local_cmds`: 客户端内置本地命令表
    /// - `remote_cmds`: ACP 后端通过 `available_commands_update` 广播的命令列表
    ///
    /// 返回:
    /// - 匹配到的 `CompletionMenu`，未触发或无候选项时返回 `None`
    pub fn build_menu(
        &mut self,
        cwd: &str,
        full_text: &str,
        text_before_cursor: &str,
        local_cmds: &[(&str, &str)],
        remote_cmds: &[SlashCommandInfo],
    ) -> Option<CompletionMenu> {
        let trigger = detect_trigger(full_text, text_before_cursor)?;
        let items = match trigger.kind {
            CompletionKind::Command => {
                let skills = self.skills_for(cwd);
                build_command_items(&trigger.query, local_cmds, remote_cmds, skills)
            }
            CompletionKind::Skill => {
                let skills = self.skills_for(cwd);
                build_skill_items(&trigger.query, skills, remote_cmds)
            }
            CompletionKind::File => {
                let files = self.files_for(cwd);
                build_file_items(&trigger.query, files)
            }
        };
        if items.is_empty() {
            return None;
        }
        Some(CompletionMenu {
            kind: trigger.kind,
            replace_start: trigger.replace_start,
            items,
        })
    }
}

/// 解析当前光标处是否处于 `/`（命令）、`#`（Skill）或 `@`（文件）补全触发态。
///
/// 参数:
/// - `full_text`: 输入框完整文本
/// - `text_before_cursor`: 光标前的文本切片
///
/// 返回:
/// - 命中任一触发符时返回 `Some(CompletionTrigger)`，否则返回 `None`
pub fn detect_trigger(full_text: &str, text_before_cursor: &str) -> Option<CompletionTrigger> {
    // 1. 检查光标所在当前词是否以 `#` 或 `@` 开头（前导必须是行首或空白，避开 URL 锚点与邮箱）
    let token_start = text_before_cursor
        .rfind(char::is_whitespace)
        .map_or(0, |idx| idx + text_before_cursor[idx..].chars().next().map_or(1, char::len_utf8));
    let token = &text_before_cursor[token_start..];
    if let Some(query) = token.strip_prefix('#') {
        return Some(CompletionTrigger {
            kind: CompletionKind::Skill,
            query: query.to_string(),
            replace_start: token_start,
        });
    }
    if let Some(query) = token.strip_prefix('@') {
        return Some(CompletionTrigger {
            kind: CompletionKind::File,
            query: query.to_string(),
            replace_start: token_start,
        });
    }

    // 2. 行首 `/` 触发命令补全（保持无换行且无空格时自动弹出）
    if full_text.starts_with('/') && !full_text.contains('\n') && !full_text.contains(' ') {
        return Some(CompletionTrigger {
            kind: CompletionKind::Command,
            query: full_text[1..].to_string(),
            replace_start: 0,
        });
    }
    None
}

/// 在发送给 ACP 后端前将 `#skill` 引用转译为后端原生可识别形式（保留正文展示给用户看原文）。
///
/// 参数:
/// - `text`: 用户输入的展开后文本
/// - `skills`: 当前工作区发现的本地 Skills 列表
/// - `remote_cmds`: ACP 后端广播的 `availableCommands` 列表
///
/// 返回:
/// - 供 `session/prompt` 发送的文本字符串
pub fn rewrite_skill_mentions(
    text: &str,
    skills: &[SkillEntry],
    remote_cmds: &[SlashCommandInfo],
) -> String {
    if !text.contains('#') {
        return text.to_string();
    }
    let remote_set: HashSet<String> = remote_cmds
        .iter()
        .map(|c| c.name.to_ascii_lowercase())
        .collect();
    let skill_map: std::collections::HashMap<String, &SkillEntry> = skills
        .iter()
        .map(|s| (s.name.to_ascii_lowercase(), s))
        .collect();

    let mut out = String::with_capacity(text.len());
    let mut local_only_refs: Vec<&SkillEntry> = Vec::new();
    let mut seen_local = HashSet::new();

    // 1. 逐词扫描独立 `#<name>`：后端已广播为斜杠命令的转为 `/<name>`，仅本地存在的附带 SKILL.md 路径指引
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut prev_is_space_or_start = true;
    while i < bytes.len() {
        let ch = text[i..].chars().next().unwrap_or('\0');
        let ch_len = ch.len_utf8();
        if ch == '#' && prev_is_space_or_start {
            let name_start = i + 1;
            let mut name_end = name_start;
            while name_end < bytes.len() {
                let c = text[name_end..].chars().next().unwrap_or('\0');
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    name_end += c.len_utf8();
                } else {
                    break;
                }
            }
            let raw_name = &text[name_start..name_end];
            let lower = raw_name.to_ascii_lowercase();
            let followed_by_boundary = name_end == bytes.len()
                || text[name_end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_whitespace() || matches!(c, ',' | '，' | '。' | ':' | '：'));
            if !raw_name.is_empty() && followed_by_boundary {
                if remote_set.contains(&lower) && skill_map.contains_key(&lower) {
                    out.push('/');
                    out.push_str(raw_name);
                    i = name_end;
                    prev_is_space_or_start = false;
                    continue;
                }
                if let Some(entry) = skill_map.get(&lower) {
                    out.push('#');
                    out.push_str(raw_name);
                    if seen_local.insert(lower) {
                        local_only_refs.push(*entry);
                    }
                    i = name_end;
                    prev_is_space_or_start = false;
                    continue;
                }
            }
        }
        out.push(ch);
        prev_is_space_or_start = ch.is_whitespace();
        i += ch_len;
    }

    // 2. 若引用了仅存在于本地目录、未被当前 ACP 后端广播的 Skill，追加其 SKILL.md 路径供后端读取
    if !local_only_refs.is_empty() {
        out.push_str("\n\n");
        for entry in local_only_refs {
            out.push_str(&format!(
                "[Skill {}: {}]\n",
                entry.name,
                entry.path.display()
            ));
        }
        while out.ends_with('\n') {
            out.pop();
        }
    }
    out
}

/// 构造 `/` 命令菜单候选项（排除已归类到 `#` 的本地已知 Skills）。
fn build_command_items(
    query: &str,
    local_cmds: &[(&str, &str)],
    remote_cmds: &[SlashCommandInfo],
    skills: &[SkillEntry],
) -> Vec<CompletionItem> {
    let q = query.trim().to_ascii_lowercase();
    let skill_names: HashSet<String> = skills
        .iter()
        .map(|s| s.name.to_ascii_lowercase())
        .collect();
    let mut out = Vec::new();
    for &(name, desc) in local_cmds {
        if q.is_empty() || name.contains(&q) || desc.to_ascii_lowercase().contains(&q) {
            out.push(CompletionItem {
                label: format!("/{name}"),
                insert_text: format!("/{name} "),
                description: desc.to_string(),
                accent_primary: true,
                command_name: Some(name.to_string()),
            });
        }
    }
    for cmd in remote_cmds {
        if local_cmds.iter().any(|(n, _)| *n == cmd.name) {
            continue;
        }
        if skill_names.contains(&cmd.name.to_ascii_lowercase()) {
            continue;
        }
        if q.is_empty()
            || cmd.name.to_ascii_lowercase().contains(&q)
            || cmd.description.to_ascii_lowercase().contains(&q)
        {
            out.push(CompletionItem {
                label: format!("/{}", cmd.name),
                insert_text: format!("/{} ", cmd.name),
                description: cmd.description.clone(),
                accent_primary: false,
                command_name: Some(cmd.name.clone()),
            });
        }
    }
    out
}

/// 构造 `#` Skills 搜索候选项，按模糊匹配度排序。
fn build_skill_items(
    query: &str,
    skills: &[SkillEntry],
    remote_cmds: &[SlashCommandInfo],
) -> Vec<CompletionItem> {
    let remote_desc: std::collections::HashMap<String, &str> = remote_cmds
        .iter()
        .map(|c| (c.name.to_ascii_lowercase(), c.description.as_str()))
        .collect();
    let mut scored: Vec<(i32, CompletionItem)> = Vec::new();
    for skill in skills {
        let lower_name = skill.name.to_ascii_lowercase();
        let in_remote = remote_desc.contains_key(&lower_name);
        let desc = if !skill.description.is_empty() {
            skill.description.clone()
        } else {
            remote_desc
                .get(&lower_name)
                .copied()
                .unwrap_or("")
                .to_string()
        };
        let score = score_skill(&skill.name, &desc, query);
        if let Some(sc) = score {
            scored.push((
                sc + if in_remote { 5 } else { 0 },
                CompletionItem {
                    label: format!("#{}", skill.name),
                    insert_text: format!("#{} ", skill.name),
                    description: desc,
                    accent_primary: in_remote,
                    command_name: None,
                },
            ));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, item)| item)
        .collect()
}

/// 构造 `@` 工作区文件搜索候选项，按文件名与路径模糊匹配打分排序。
fn build_file_items(query: &str, files: &[String]) -> Vec<CompletionItem> {
    let mut scored: Vec<(i32, &str)> = files
        .iter()
        .filter_map(|path| score_file_path(path, query).map(|s| (s, path.as_str())))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(b.1))
    });
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, path)| {
            let (file_name, parent_dir) = split_file_label(path);
            let insert_text = if path.contains(char::is_whitespace) {
                format!("@\"{path}\" ")
            } else {
                format!("@{path} ")
            };
            CompletionItem {
                label: format!("@{file_name}"),
                insert_text,
                description: parent_dir,
                accent_primary: true,
                command_name: None,
            }
        })
        .collect()
}

fn split_file_label(rel_path: &str) -> (&str, String) {
    match rel_path.rsplit_once('/') {
        Some((dir, file)) => (file, dir.to_string()),
        None => (rel_path, ".".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_command_skill_and_file_triggers_without_email_or_url_false_positives() {
        assert_eq!(
            detect_trigger("/mod", "/mod"),
            Some(CompletionTrigger {
                kind: CompletionKind::Command,
                query: "mod".into(),
                replace_start: 0,
            })
        );
        assert_eq!(
            detect_trigger("请用 #code-re", "请用 #code-re"),
            Some(CompletionTrigger {
                kind: CompletionKind::Skill,
                query: "code-re".into(),
                replace_start: "请用 ".len(),
            })
        );
        assert_eq!(
            detect_trigger("看看 @src/ma 吧", "看看 @src/ma"),
            Some(CompletionTrigger {
                kind: CompletionKind::File,
                query: "src/ma".into(),
                replace_start: "看看 ".len(),
            })
        );
        // 邮箱和 URL 锚点不能误触发
        assert_eq!(detect_trigger("foo@bar.com", "foo@bar.com"), None);
        assert_eq!(detect_trigger("https://a.io#top", "https://a.io#top"), None);
    }

    #[test]
    fn separates_skills_from_slash_commands_and_rewrites_mentions_for_backend() {
        let skills = vec![
            SkillEntry {
                name: "code-review".into(),
                description: "Review changes".into(),
                path: PathBuf::from("/skills/code-review/SKILL.md"),
            },
            SkillEntry {
                name: "local-only".into(),
                description: "Local helper".into(),
                path: PathBuf::from("/skills/local-only/SKILL.md"),
            },
        ];
        let remote_cmds = vec![
            SlashCommandInfo {
                name: "compact".into(),
                description: "Compact context".into(),
            },
            SlashCommandInfo {
                name: "code-review".into(),
                description: "Review changes".into(),
            },
        ];

        // 1. `/` 命令列表应过滤掉已归入 `#` 的 code-review，保留 compact 与本地命令
        let cmds = build_command_items("", &[("model", "切换模型")], &remote_cmds, &skills);
        assert!(cmds.iter().any(|c| c.label == "/model"));
        assert!(cmds.iter().any(|c| c.label == "/compact"));
        assert!(!cmds.iter().any(|c| c.label == "/code-review"));

        // 2. `#` 列表包含 code-review 与 local-only
        let skill_items = build_skill_items("rev", &skills, &remote_cmds);
        assert_eq!(skill_items.len(), 1);
        assert_eq!(skill_items[0].label, "#code-review");
        assert_eq!(skill_items[0].insert_text, "#code-review ");

        // 3. 发送转译：后端广播过的转 `/code-review`，仅本地存在的附带 SKILL.md 路径
        let rewritten = rewrite_skill_mentions(
            "#code-review 检查一下 并结合 #local-only",
            &skills,
            &remote_cmds,
        );
        assert!(rewritten.starts_with("/code-review 检查一下"));
        assert!(rewritten.contains("[Skill local-only: /skills/local-only/SKILL.md]"));
    }

    #[test]
    fn scans_skills_and_fuzzy_matches_workspace_files() {
        let root = std::env::temp_dir().join(format!(
            "agy_tui_completion_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".agents/skills/my-skill")).unwrap();
        std::fs::create_dir_all(root.join("src/ui")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(
            root.join(".agents/skills/my-skill/SKILL.md"),
            "---\nname: my-skill\ndescription: \"Demo skill for test\"\n---\n# Body\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("src/ui/sidebar.rs"), "").unwrap();
        std::fs::write(root.join("target/debug/ignored.rs"), "").unwrap();

        let found_skills = scan::discover_skills_in_roots(&[root.join(".agents/skills")]);
        assert_eq!(found_skills.len(), 1);
        assert_eq!(found_skills[0].name, "my-skill");
        assert_eq!(found_skills[0].description, "Demo skill for test");

        let files = scan_workspace_files(root.to_str().unwrap());
        assert!(files.iter().any(|f| f == "src/main.rs"));
        assert!(files.iter().any(|f| f == "src/ui/sidebar.rs"));
        assert!(!files.iter().any(|f| f.starts_with("target/")));

        let hits = build_file_items("sbar", &files);
        assert_eq!(hits[0].insert_text, "@src/ui/sidebar.rs ");
        let _ = std::fs::remove_dir_all(&root);
    }
}
