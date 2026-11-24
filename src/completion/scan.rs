//! Skills 目录扫描（`SKILL.md` frontmatter 解析）、工作区文件遍历与模糊匹配打分。

use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::SkillEntry;

const MAX_INDEXED_FILES: usize = 4000;
const MAX_SCAN_DEPTH: usize = 8;

/// 常见构建产物与缓存目录黑名单，避免遍历耗时目录。
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".idea",
    ".vscode",
    ".cache",
    ".gradle",
    ".cargo",
];

/// 扫描跨 Agent 通用 Skills 目录（项目级优先、其次用户级与内置目录），解析 `SKILL.md` frontmatter。
pub fn discover_skills(cwd: &str) -> Vec<SkillEntry> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let cwd_path = Path::new(cwd);
    for rel in [".agents/skills", ".claude/skills", ".gemini/skills"] {
        roots.push(cwd_path.join(rel));
    }
    if let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
    {
        for rel in [
            ".agents/skills",
            ".claude/skills",
            ".gemini/skills",
            ".gemini/antigravity-cli/builtin/skills",
        ] {
            roots.push(home.join(rel));
        }
    }
    discover_skills_in_roots(&roots)
}

pub(super) fn discover_skills_in_roots(roots: &[PathBuf]) -> Vec<SkillEntry> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        children.sort();
        for dir in children {
            let skill_md = dir.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let fallback_name = dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (name, description) = parse_skill_frontmatter(&skill_md, &fallback_name);
            if name.is_empty() {
                continue;
            }
            let key = name.to_ascii_lowercase();
            if seen.insert(key) {
                out.push(SkillEntry {
                    name,
                    description,
                    path: skill_md,
                });
            }
        }
    }
    out
}

/// 读取 `SKILL.md` 头部最多 4KB 并解析 YAML frontmatter 中的 `name` 与 `description`。
fn parse_skill_frontmatter(path: &Path, fallback_name: &str) -> (String, String) {
    let Ok(file) = std::fs::File::open(path) else {
        return (fallback_name.to_string(), String::new());
    };
    let mut buf = String::new();
    let _ = file.take(4096).read_to_string(&mut buf);
    let trimmed = buf.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
    else {
        return (fallback_name.to_string(), String::new());
    };

    let mut name = String::new();
    let mut description = String::new();
    let mut in_desc_block = false;

    for raw_line in rest.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim() == "---" {
            break;
        }
        if in_desc_block {
            if line.starts_with(' ') || line.starts_with('\t') {
                let piece = line.trim();
                if !piece.is_empty() {
                    if !description.is_empty() {
                        description.push(' ');
                    }
                    description.push_str(piece);
                }
                continue;
            }
            in_desc_block = false;
        }
        if let Some(val) = line.strip_prefix("name:") {
            name = unquote_yaml_scalar(val);
        } else if let Some(val) = line.strip_prefix("description:") {
            let v = val.trim();
            if matches!(v, "|" | ">" | "|-" | ">-") {
                in_desc_block = true;
            } else {
                description = unquote_yaml_scalar(v);
            }
        }
    }

    if name.is_empty() {
        name = fallback_name.to_string();
    }
    (name, description)
}

fn unquote_yaml_scalar(raw: &str) -> String {
    let s = raw.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        return s[1..s.len() - 1].trim().to_string();
    }
    s.to_string()
}

/// 广度优先扫描工作区文件（自动跳过常见构建/缓存目录与根目录 `.gitignore` 简单目录项）。
pub fn scan_workspace_files(cwd: &str) -> Vec<String> {
    let root = Path::new(cwd);
    if !root.is_dir() {
        return Vec::new();
    }
    let extra_ignored = read_simple_gitignore_dirs(root);
    let mut files = Vec::new();
    let mut queue: VecDeque<(PathBuf, String, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), String::new(), 0));

    while let Some((dir, rel_prefix, depth)) = queue.pop_front() {
        if files.len() >= MAX_INDEXED_FILES {
            break;
        }
        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = read_dir.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if files.len() >= MAX_INDEXED_FILES {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.is_empty() || name == ".DS_Store" {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            // 跳过符号链接防止死循环
            if ft.is_symlink() {
                continue;
            }
            let rel = if rel_prefix.is_empty() {
                name.clone()
            } else {
                format!("{rel_prefix}/{name}")
            };
            if ft.is_dir() {
                if depth < MAX_SCAN_DEPTH
                    && !IGNORED_DIRS.contains(&name.as_str())
                    && !extra_ignored.contains(&name)
                    && !extra_ignored.contains(&rel)
                {
                    queue.push_back((entry.path(), rel, depth + 1));
                }
            } else if ft.is_file() {
                files.push(rel);
            }
        }
    }
    files
}

fn read_simple_gitignore_dirs(root: &Path) -> HashSet<String> {
    let mut set = HashSet::new();
    let Ok(content) = std::fs::read_to_string(root.join(".gitignore")) else {
        return set;
    };
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with('!')
            || line.contains('*')
            || line.contains('?')
            || line.contains('[')
        {
            continue;
        }
        let cleaned = line.trim_start_matches('/').trim_end_matches('/');
        if !cleaned.is_empty() {
            set.insert(cleaned.to_string());
        }
    }
    set
}

pub(super) fn score_skill(name: &str, desc: &str, query: &str) -> Option<i32> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Some(100);
    }
    let name_l = name.to_ascii_lowercase();
    if name_l == q {
        return Some(1000);
    }
    if name_l.starts_with(&q) {
        return Some(800 - (name_l.len() as i32));
    }
    if let Some(pos) = name_l.find(&q) {
        return Some(600 - (pos as i32 * 4));
    }
    if let Some(sc) = fuzzy_subsequence_score(&name_l, &q) {
        return Some(400 + sc);
    }
    let desc_l = desc.to_ascii_lowercase();
    if desc_l.contains(&q) {
        return Some(200);
    }
    None
}

pub(super) fn score_file_path(path: &str, query: &str) -> Option<i32> {
    let q = query.trim().replace('\\', "/").to_ascii_lowercase();
    if q.is_empty() {
        let depth = path.matches('/').count() as i32;
        return Some(100 - depth * 5);
    }
    let path_l = path.to_ascii_lowercase();
    let file_l = path_l.rsplit('/').next().unwrap_or(&path_l);
    if file_l == q {
        return Some(1000);
    }
    if file_l.starts_with(&q) {
        return Some(850 - (file_l.len() as i32));
    }
    if let Some(pos) = file_l.find(&q) {
        return Some(700 - (pos as i32 * 3));
    }
    if let Some(pos) = path_l.find(&q) {
        return Some(550 - (pos as i32 * 2));
    }
    if let Some(sc) = fuzzy_subsequence_score(file_l, &q) {
        return Some(450 + sc);
    }
    fuzzy_subsequence_score(&path_l, &q).map(|sc| 250 + sc)
}

/// 轻量字符子序列模糊匹配打分（连续匹配和路径分隔边界加分）。
fn fuzzy_subsequence_score(haystack: &str, needle: &str) -> Option<i32> {
    let mut h_chars = haystack.chars().enumerate().peekable();
    let mut score = 0i32;
    let mut prev_match: Option<usize> = None;
    let mut prev_char = '/';

    for nc in needle.chars() {
        let mut matched = None;
        for (idx, hc) in h_chars.by_ref() {
            if hc == nc {
                matched = Some((idx, prev_char));
                prev_char = hc;
                break;
            }
            prev_char = hc;
        }
        let (idx, before) = matched?;
        score += 10;
        if prev_match.is_some_and(|p| idx == p + 1) {
            score += 12;
        }
        if matches!(before, '/' | '_' | '-' | '.') {
            score += 8;
        }
        prev_match = Some(idx);
    }
    Some(score - (haystack.len() as i32 / 4))
}
