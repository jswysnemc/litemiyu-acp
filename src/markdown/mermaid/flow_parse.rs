//! Mermaid 流程图（`flowchart` / `graph`）IR 与语法解析器。
//! 负责将流程图源码解析为节点与有向边集合，供 `flowchart.rs` 布局渲染与 `state.rs` 复用。

use std::collections::HashMap;

/// 流程图布局方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
    TopDown,
    BottomUp,
}

/// 节点外观形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    Rect,
    Rounded,
    Diamond,
    Cylinder,
    Circle,
    StartEnd,
}

/// 边线风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeStyle {
    Solid,
    Dashed,
    Thick,
    Undirected,
}

/// 解析后的图节点。
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub label: String,
    pub shape: NodeShape,
}

/// 解析后的图有向边。
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub style: EdgeStyle,
}

/// 通用流程图 IR（供 `flowchart` 与 `state` 共用）。
#[derive(Debug, Clone)]
pub struct FlowGraph {
    pub direction: Direction,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    id_map: HashMap<String, usize>,
}

impl FlowGraph {
    /// 创建空图。
    ///
    /// 参数:
    /// - `direction`: 期望的布局方向
    ///
    /// 返回:
    /// - 空 `FlowGraph`
    pub fn new(direction: Direction) -> Self {
        Self {
            direction,
            nodes: Vec::new(),
            edges: Vec::new(),
            id_map: HashMap::new(),
        }
    }

    /// 查找或插入节点；若新提供更具体的标签或形状则更新。
    ///
    /// 参数:
    /// - `id`: 节点唯一标识
    /// - `label`: 可选显示文本（为空时回退 `id`）
    /// - `shape`: 可选节点形状
    ///
    /// 返回:
    /// - 节点在 `nodes` 中的索引
    pub fn upsert_node(
        &mut self,
        id: &str,
        label: Option<String>,
        shape: Option<NodeShape>,
    ) -> usize {
        if let Some(&idx) = self.id_map.get(id) {
            if let Some(lbl) = label
                && !lbl.is_empty()
            {
                self.nodes[idx].label = lbl;
            }
            if let Some(sh) = shape {
                self.nodes[idx].shape = sh;
            }
            return idx;
        }
        let idx = self.nodes.len();
        let display = label
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.to_string());
        self.nodes.push(GraphNode {
            label: display,
            shape: shape.unwrap_or(NodeShape::Rect),
        });
        self.id_map.insert(id.to_string(), idx);
        idx
    }

    /// 新增一条有向/无向边。
    ///
    /// 参数:
    /// - `from`: 起点节点索引
    /// - `to`: 终点节点索引
    /// - `label`: 可选边标签
    /// - `style`: 边线风格
    pub fn add_edge(&mut self, from: usize, to: usize, label: Option<String>, style: EdgeStyle) {
        self.edges.push(GraphEdge {
            from,
            to,
            label: label
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            style,
        });
    }
}

/// 解析 Mermaid `flowchart` / `graph` 文本为 `FlowGraph`。
///
/// 参数:
/// - `source`: 完整 Mermaid 源码
///
/// 返回:
/// - 解析成功且至少含一个节点时返回 `Some(FlowGraph)`，否则返回 `None`
pub fn parse(source: &str) -> Option<FlowGraph> {
    let mut lines = source
        .lines()
        .map(strip_comment)
        .map(str::trim)
        .filter(|l| !l.is_empty());

    let header = lines.next()?;
    let mut tokens = header.split_whitespace();
    let kind = tokens.next()?.to_ascii_lowercase();
    if kind != "flowchart" && kind != "graph" {
        return None;
    }
    let dir_token = tokens.next().unwrap_or("TD").to_ascii_uppercase();
    let direction = match dir_token.as_str() {
        "LR" => Direction::LeftToRight,
        "RL" => Direction::RightToLeft,
        "BT" => Direction::BottomUp,
        _ => Direction::TopDown,
    };

    let mut graph = FlowGraph::new(direction);
    for raw_line in lines {
        for stmt in split_statements(raw_line) {
            let s = stmt.trim();
            if s.is_empty() || is_ignored_directive(s) {
                continue;
            }
            parse_statement(&mut graph, s);
        }
    }
    (!graph.nodes.is_empty()).then_some(graph)
}

fn strip_comment(line: &str) -> &str {
    line.find("%%").map_or(line, |i| &line[..i])
}

fn is_ignored_directive(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower == "end"
        || lower.starts_with("subgraph ")
        || lower == "subgraph"
        || lower.starts_with("classdef ")
        || lower.starts_with("class ")
        || lower.starts_with("style ")
        || lower.starts_with("linkstyle ")
        || lower.starts_with("click ")
        || lower.starts_with("direction ")
}

/// 按分号切分单行多语句（忽略引号与括号内部的分号）。
fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let mut depth = 0i32;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            '[' | '(' | '{' if !in_quote => depth += 1,
            ']' | ')' | '}' if !in_quote => depth = (depth - 1).max(0),
            ';' if !in_quote && depth == 0 => {
                out.push(&line[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    if start <= line.len() {
        out.push(&line[start..]);
    }
    out
}

/// 解析单条流程图语句（支持链式 `A --> B --> C` 与 `&` 并行连接）。
fn parse_statement(graph: &mut FlowGraph, stmt: &str) {
    let mut segments: Vec<String> = Vec::new();
    let mut connectors: Vec<(EdgeStyle, Option<String>)> = Vec::new();
    let mut rest = stmt.trim();

    while let Some((left, style, label, right)) = split_next_edge(rest) {
        segments.push(left.trim().to_string());
        connectors.push((style, label));
        rest = right.trim();
    }
    if !rest.is_empty() {
        segments.push(rest.to_string());
    }
    if segments.is_empty() {
        return;
    }

    let node_groups: Vec<Vec<usize>> = segments
        .iter()
        .map(|seg| parse_node_group(graph, seg))
        .collect();

    for (idx, (style, label)) in connectors.into_iter().enumerate() {
        let froms = &node_groups[idx];
        let tos = node_groups.get(idx + 1).map(Vec::as_slice).unwrap_or(&[]);
        for &u in froms {
            for &v in tos {
                graph.add_edge(u, v, label.clone(), style);
            }
        }
    }
}

/// 在语句顶层（引号与括号外）查找下一处边连接符。
fn split_next_edge(input: &str) -> Option<(&str, EdgeStyle, Option<String>, &str)> {
    let bytes = input.as_bytes();
    let mut in_quote = false;
    let mut depth = 0i32;
    let mut i = 0usize;

    while i < bytes.len() {
        let ch = input[i..].chars().next()?;
        if ch == '"' {
            in_quote = !in_quote;
            i += ch.len_utf8();
            continue;
        }
        if !in_quote {
            match ch {
                '[' | '(' | '{' => depth += 1,
                ']' | ')' | '}' => depth = (depth - 1).max(0),
                _ if depth == 0 => {
                    if let Some((style, inline_label, consumed)) = match_edge_token(&input[i..]) {
                        let left = &input[..i];
                        if !left.trim().is_empty() {
                            let after = &input[i + consumed..];
                            let (pipe_label, right) = extract_pipe_label(after);
                            let label = pipe_label.or(inline_label);
                            return Some((left, style, label, right));
                        }
                    }
                }
                _ => {}
            }
        }
        i += ch.len_utf8();
    }
    None
}

/// 识别各类 Mermaid 边运算符及内联标签（如 `-- 文字 -->`、`-. 文字 .->`、`== 文字 ==>`）。
fn match_edge_token(s: &str) -> Option<(EdgeStyle, Option<String>, usize)> {
    // 1. 先匹配固定符号形式（长符号优先）
    const FIXED: &[(&str, EdgeStyle)] = &[
        ("<-->", EdgeStyle::Solid),
        ("-.->", EdgeStyle::Dashed),
        ("==>", EdgeStyle::Thick),
        ("-->", EdgeStyle::Solid),
        ("---", EdgeStyle::Undirected),
        ("-.-", EdgeStyle::Dashed),
        ("===", EdgeStyle::Thick),
    ];
    for &(tok, style) in FIXED {
        if s.starts_with(tok) {
            return Some((style, None, tok.len()));
        }
    }
    // 2. 匹配内联标签形式：`-- text -->` / `-. text .->` / `== text ==>`
    if let Some(rest) = s.strip_prefix("-- ")
        && let Some(end) = rest.find("-->")
    {
        let lbl = rest[..end].trim().to_string();
        return Some((EdgeStyle::Solid, Some(lbl), 3 + end + 3));
    }
    if let Some(rest) = s.strip_prefix("-. ")
        && let Some(end) = rest.find(".->")
    {
        let lbl = rest[..end].trim().to_string();
        return Some((EdgeStyle::Dashed, Some(lbl), 3 + end + 3));
    }
    if let Some(rest) = s.strip_prefix("== ")
        && let Some(end) = rest.find("==>")
    {
        let lbl = rest[..end].trim().to_string();
        return Some((EdgeStyle::Thick, Some(lbl), 3 + end + 3));
    }
    None
}

/// 提取紧随边后的 `|标签|`。
fn extract_pipe_label(s: &str) -> (Option<String>, &str) {
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed.strip_prefix('|')
        && let Some(end) = rest.find('|')
    {
        let lbl = clean_label(&rest[..end]);
        return (Some(lbl), &rest[end + 1..]);
    }
    (None, s)
}

/// 解析以 `&` 分隔的一组节点表达式。
fn parse_node_group(graph: &mut FlowGraph, seg: &str) -> Vec<usize> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let mut depth = 0i32;
    for (idx, ch) in seg.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            '[' | '(' | '{' if !in_quote => depth += 1,
            ']' | ')' | '}' if !in_quote => depth = (depth - 1).max(0),
            '&' if !in_quote && depth == 0 => {
                parts.push(&seg[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&seg[start..]);

    parts
        .into_iter()
        .filter_map(|part| parse_single_node(graph, part.trim()))
        .collect()
}

/// 解析单个节点表达式（如 `A[用户输入]`、`C{响应类型}`、`D([胶囊])`）。
fn parse_single_node(graph: &mut FlowGraph, raw: &str) -> Option<usize> {
    let token = raw.trim();
    if token.is_empty() {
        return None;
    }
    let token = token.split(":::").next().unwrap_or(token).trim();
    let delim_pos = token.find(['[', '(', '{']);
    let Some(pos) = delim_pos else {
        let id = token.split_whitespace().next()?;
        return Some(graph.upsert_node(id, None, None));
    };
    let id = token[..pos].trim();
    if id.is_empty() {
        return None;
    }
    let body = token[pos..].trim();
    let (shape, inner) = extract_node_shape(body);
    let label = clean_label(inner);
    Some(graph.upsert_node(id, Some(label), Some(shape)))
}

fn extract_node_shape(body: &str) -> (NodeShape, &str) {
    let patterns: &[(&str, &str, NodeShape)] = &[
        ("([", "])", NodeShape::Rounded),
        ("[(", ")]", NodeShape::Cylinder),
        ("((", "))", NodeShape::Circle),
        ("[[", "]]", NodeShape::Rect),
        ("{{", "}}", NodeShape::Diamond),
        ("[/", "/]", NodeShape::Rect),
        ("[\\", "\\]", NodeShape::Rect),
        ("[", "]", NodeShape::Rect),
        ("(", ")", NodeShape::Rounded),
        ("{", "}", NodeShape::Diamond),
    ];
    for &(open, close, shape) in patterns {
        if let Some(rest) = body.strip_prefix(open)
            && let Some(inner) = rest.strip_suffix(close)
        {
            return (shape, inner);
        }
    }
    (NodeShape::Rect, body)
}

fn clean_label(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted
        .replace("<br/>", " / ")
        .replace("<br />", " / ")
        .replace("<br>", " / ")
        .replace("\\n", " / ")
        .trim()
        .to_string()
}
