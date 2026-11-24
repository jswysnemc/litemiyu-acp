//! Mermaid 状态图（`stateDiagram` / `stateDiagram-v2`）解析器。
//! 将状态与流转转换为 `FlowGraph`，直接复用 `flowchart::render_graph` 的布局与框线引擎。

use super::flow_parse::{Direction, EdgeStyle, FlowGraph, NodeShape};
use super::flowchart::render_graph;
use crate::markdown::inline::Folded;

/// 尝试将 Mermaid `stateDiagram` / `stateDiagram-v2` 源码渲染为 Unicode 框线行集合。
///
/// 参数:
/// - `source`: 完整 Mermaid 源码
/// - `width`: 可用内容列宽
///
/// 返回:
/// - 成功渲染时返回 `Some(Vec<Folded>)`，无法解析时返回 `None`
pub fn render(source: &str, width: usize) -> Option<Vec<Folded>> {
    let graph = parse(source)?;
    render_graph(&graph, width)
}

fn parse(source: &str) -> Option<FlowGraph> {
    let mut lines = source
        .lines()
        .map(|l| l.find("%%").map_or(l, |i| &l[..i]))
        .map(str::trim)
        .filter(|l| !l.is_empty());

    let first = lines.next()?;
    let lower_first = first.to_ascii_lowercase();
    if lower_first != "statediagram" && lower_first != "statediagram-v2" {
        return None;
    }

    let mut graph = FlowGraph::new(Direction::TopDown);
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if lower == "{" || lower == "}" || lower.starts_with("note ") {
            continue;
        }
        if let Some(rest) = strip_prefix_ci(line, "direction ") {
            graph.direction = match rest.trim().to_ascii_uppercase().as_str() {
                "LR" => Direction::LeftToRight,
                "RL" => Direction::RightToLeft,
                "BT" => Direction::BottomUp,
                _ => Direction::TopDown,
            };
            continue;
        }
        // `state "运行中" as Running`
        if let Some(rest) = strip_prefix_ci(line, "state ") {
            if let Some((lbl, id)) = rest.split_once(" as ") {
                let clean_lbl = lbl.trim().trim_matches('"').to_string();
                let clean_id = id.trim();
                if !clean_id.is_empty() {
                    graph.upsert_node(clean_id, Some(clean_lbl), Some(NodeShape::Rounded));
                }
            }
            continue;
        }
        // 状态转移 `A --> B : 条件`
        if let Some((lhs, rhs)) = line.split_once("-->") {
            let from_raw = lhs.trim();
            let (to_raw, label) = match rhs.split_once(':') {
                Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
                None => (rhs.trim(), None),
            };
            if from_raw.is_empty() || to_raw.is_empty() {
                continue;
            }
            let from_idx = upsert_state_endpoint(&mut graph, from_raw, true);
            let to_idx = upsert_state_endpoint(&mut graph, to_raw, false);
            graph.add_edge(from_idx, to_idx, label, EdgeStyle::Solid);
            continue;
        }
        // 状态描述 `Idle : 空闲`
        if let Some((id, desc)) = line.split_once(':') {
            let clean_id = id.trim();
            let clean_desc = desc.trim();
            if !clean_id.is_empty() && !clean_desc.is_empty() {
                graph.upsert_node(
                    clean_id,
                    Some(format!("{clean_id} ({clean_desc})")),
                    Some(NodeShape::Rounded),
                );
            }
        }
    }

    (!graph.nodes.is_empty()).then_some(graph)
}

fn upsert_state_endpoint(graph: &mut FlowGraph, raw: &str, is_source: bool) -> usize {
    if raw == "[*]" {
        if is_source {
            graph.upsert_node("__start__", Some("●".into()), Some(NodeShape::StartEnd))
        } else {
            graph.upsert_node("__end__", Some("◉".into()), Some(NodeShape::StartEnd))
        }
    } else {
        graph.upsert_node(raw, None, Some(NodeShape::Rounded))
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::inline::plain;

    #[test]
    fn state_diagram_renders_start_end_and_transitions() {
        let src = "\
stateDiagram-v2
    [*] --> 空闲
    空闲 --> 生成中 : 发送提问
    生成中 --> [*] : 完成
";
        let rows = render(src, 64).expect("should render state diagram");
        let joined = rows.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(joined.contains('●'));
        assert!(joined.contains("空闲"));
        assert!(joined.contains("生成中"));
        assert!(joined.contains('◉'));
    }
}
