//! 纯 Rust 轻量 Unicode 框线 Mermaid 渲染入口。
//! 支持 `flowchart`/`graph`、`sequenceDiagram`、`stateDiagram`/`stateDiagram-v2`；
//! 暂不支持的图表类型或解析失败时返回 `None`，由调用方平滑回退为带词法高亮的代码块。

mod canvas;
mod flow_parse;
mod flowchart;
mod sequence;
mod state;

use crate::markdown::inline::Folded;

/// 判断代码块语言标识是否为 Mermaid。
///
/// 参数:
/// - `lang`: Markdown 代码块语言标签
///
/// 返回:
/// - 匹配 `mermaid` 或 `mmd`（忽略大小写）时返回 `true`
pub fn is_mermaid_lang(lang: &str) -> bool {
    matches!(lang.trim().to_ascii_lowercase().as_str(), "mermaid" | "mmd")
}

/// 尝试将 Mermaid 源码渲染为终端 Unicode 框线行集合。
///
/// 参数:
/// - `source`: Mermaid 代码块正文
/// - `width`: 可用内容列宽
///
/// 返回:
/// - 成功渲染时返回 `Some(Vec<Folded>)`，否则返回 `None` 以回退源码展示
pub fn render(source: &str, width: usize) -> Option<Vec<Folded>> {
    let first_line = source
        .lines()
        .map(|l| l.find("%%").map_or(l, |i| &l[..i]))
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    let head = first_line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    match head.as_str() {
        "flowchart" | "graph" => flowchart::render(source, width),
        "sequencediagram" => sequence::render(source, width),
        "statediagram" | "statediagram-v2" => state::render(source, width),
        _ => None,
    }
}
