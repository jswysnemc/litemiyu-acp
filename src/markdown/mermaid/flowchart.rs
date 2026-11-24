//! Mermaid 流程图（`flowchart` / `graph`）拓扑分层与 Unicode 框线渲染器。
//! 支持宽屏横向分层与窄屏纵向自适应降级，并在框线与中文宽字之间保持严格列对齐。

use std::collections::VecDeque;

use unicode_width::UnicodeWidthStr;

use super::canvas::{Canvas, DIR_DOWN, DIR_LEFT, DIR_RIGHT, DIR_UP, clip_width};
use super::flow_parse::{Direction, EdgeStyle, FlowGraph, GraphEdge, GraphNode, NodeShape, parse};
use crate::markdown::inline::Folded;
use crate::theme;

/// 尝试将 Mermaid `flowchart` / `graph` 源码渲染为 Unicode 框线行集合。
///
/// 参数:
/// - `source`: 完整 Mermaid 源码
/// - `width`: 可用内容列宽
///
/// 返回:
/// - 成功解析并渲染时返回 `Some(Vec<Folded>)`，无法解析时返回 `None` 以便回退代码块
pub fn render(source: &str, width: usize) -> Option<Vec<Folded>> {
    let graph = parse(source)?;
    render_graph(&graph, width)
}

/// 将已构建的 `FlowGraph` 渲染为 Unicode 框线行集合（供 `state.rs` 复用）。
///
/// 参数:
/// - `graph`: 流程图 IR
/// - `width`: 可用内容列宽
///
/// 返回:
/// - 渲染后的 `Folded` 行集合；空图或宽度过窄时返回 `None`
pub fn render_graph(graph: &FlowGraph, width: usize) -> Option<Vec<Folded>> {
    if graph.nodes.is_empty() || width < 16 {
        return None;
    }
    // 1. 拓扑分层，计算每个节点所在层级
    let mut layers = assign_layers(graph);
    if layers.is_empty() {
        return None;
    }
    let reverse_order = matches!(graph.direction, Direction::RightToLeft | Direction::BottomUp);
    if reverse_order {
        layers.reverse();
    }

    // 2. 优先按声明方向排版；横排超出可用宽度时自动降级为纵向分层
    let want_horizontal = matches!(
        graph.direction,
        Direction::LeftToRight | Direction::RightToLeft
    );
    if want_horizontal
        && let Some(rows) = render_horizontal(graph, &layers, width, reverse_order)
    {
        return Some(rows);
    }
    render_vertical(graph, &layers, width, reverse_order)
}

/// 按最长路径（容忍环）为各节点分配拓扑层级。
fn assign_layers(graph: &FlowGraph) -> Vec<Vec<usize>> {
    let n = graph.nodes.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for e in &graph.edges {
        if e.from != e.to && !adj[e.from].contains(&e.to) {
            adj[e.from].push(e.to);
            indeg[e.to] += 1;
        }
    }

    let mut layer = vec![0usize; n];
    let mut queue = VecDeque::new();
    for (i, &d) in indeg.iter().enumerate() {
        if d == 0 {
            queue.push_back(i);
        }
    }
    if queue.is_empty() && n > 0 {
        queue.push_back(0);
    }

    let mut visited = 0usize;
    let mut processed = vec![false; n];
    while let Some(u) = queue.pop_front() {
        if processed[u] {
            continue;
        }
        processed[u] = true;
        visited += 1;
        for &v in &adj[u] {
            if !processed[v] {
                layer[v] = layer[v].max(layer[u] + 1);
                indeg[v] = indeg[v].saturating_sub(1);
                if indeg[v] == 0 {
                    queue.push_back(v);
                }
            }
        }
    }
    if visited < n {
        for u in 0..n {
            if !processed[u] {
                for &v in &adj[u] {
                    if !processed[v] {
                        layer[v] = layer[v].max(layer[u] + 1);
                    }
                }
            }
        }
    }

    let max_layer = layer.iter().copied().max().unwrap_or(0);
    let mut layers = vec![Vec::new(); max_layer + 1];
    for (idx, &l) in layer.iter().enumerate() {
        layers[l].push(idx);
    }
    layers.retain(|l| !l.is_empty());
    layers
}

/// 格式化节点显示文本（为菱形、数据库、起止点加入紧凑语义标识）。
fn formatted_node_text(node: &GraphNode, max_inner_w: usize) -> String {
    let raw = match node.shape {
        NodeShape::Diamond => format!("◇ {}", node.label),
        NodeShape::Cylinder => format!("⌸ {}", node.label),
        NodeShape::StartEnd => node.label.clone(),
        _ => node.label.clone(),
    };
    clip_width(&raw, max_inner_w.max(2))
}

/// 计算单节点盒子含边框的列宽。
fn node_box_width(node: &GraphNode, max_box_w: usize) -> usize {
    if node.shape == NodeShape::StartEnd && (node.label == "●" || node.label == "◉") {
        return 5;
    }
    let inner_cap = max_box_w.saturating_sub(4).max(4);
    let text = formatted_node_text(node, inner_cap);
    (text.width() + 4).clamp(6, max_box_w.max(6))
}

/// 在画布 `(x, y)` 处绘制节点方框与标签。
fn draw_node(canvas: &mut Canvas, node: &GraphNode, x: usize, y: usize, box_w: usize) {
    let border_style = match node.shape {
        NodeShape::Diamond => theme::warn(),
        NodeShape::StartEnd => theme::good(),
        NodeShape::Cylinder => theme::thought(),
        _ => theme::accent(),
    };
    let text_style = match node.shape {
        NodeShape::StartEnd => theme::good(),
        NodeShape::Diamond => theme::md_bold(),
        _ => theme::text(),
    };
    let rounded = matches!(
        node.shape,
        NodeShape::Rounded
            | NodeShape::Diamond
            | NodeShape::Circle
            | NodeShape::Cylinder
            | NodeShape::StartEnd
    );
    canvas.draw_box(x, y, box_w, 3, rounded, border_style);
    let inner_w = box_w.saturating_sub(4).max(1);
    let text = formatted_node_text(node, inner_w);
    canvas.put_centered(x + 2, y + 1, inner_w, &text, text_style);
}

/// 横向分层布局（适用于 `LR` / `RL` 且屏幕宽度足够容纳全部层时）。
fn render_horizontal(
    graph: &FlowGraph,
    layers: &[Vec<usize>],
    max_width: usize,
    reverse: bool,
) -> Option<Vec<Folded>> {
    let num_layers = layers.len();
    let max_node_w = 24usize;
    let col_widths: Vec<usize> = layers
        .iter()
        .map(|col| {
            col.iter()
                .map(|&idx| node_box_width(&graph.nodes[idx], max_node_w))
                .max()
                .unwrap_or(6)
        })
        .collect();

    let mut node_layer = vec![0usize; graph.nodes.len()];
    for (li, col) in layers.iter().enumerate() {
        for &idx in col {
            node_layer[idx] = li;
        }
    }

    let mut gaps = vec![4usize; num_layers.saturating_sub(1)];
    for e in &graph.edges {
        let (la, lb) = (node_layer[e.from], node_layer[e.to]);
        if la.abs_diff(lb) == 1 {
            let g = la.min(lb);
            if let Some(lbl) = &e.label {
                let need = lbl.width() + 6;
                gaps[g] = gaps[g].max(need.min(20));
            }
        }
    }

    let total_w: usize = col_widths.iter().sum::<usize>() + gaps.iter().sum::<usize>();
    if total_w > max_width {
        return None;
    }

    let max_rows_in_col = layers.iter().map(Vec::len).max().unwrap_or(1);
    let row_pitch = 4usize;
    let has_any_label = graph.edges.iter().any(|e| e.label.is_some());
    let top_margin = usize::from(has_any_label);
    let canvas_h = top_margin + max_rows_in_col * row_pitch;
    let mut col_x = Vec::with_capacity(num_layers);
    let mut cur_x = 0usize;
    for (li, &cw) in col_widths.iter().enumerate() {
        col_x.push(cur_x);
        cur_x += cw;
        if let Some(&g) = gaps.get(li) {
            cur_x += g;
        }
    }

    let mut pos: Vec<(usize, usize, usize)> = vec![(0, 0, 0); graph.nodes.len()];
    for (li, col) in layers.iter().enumerate() {
        let group_h = col.len() * row_pitch - 1;
        let top_y = top_margin + (max_rows_in_col * row_pitch).saturating_sub(group_h) / 2;
        for (ri, &nid) in col.iter().enumerate() {
            let bw = node_box_width(&graph.nodes[nid], max_node_w);
            let x = col_x[li] + col_widths[li].saturating_sub(bw) / 2;
            let y = top_y + ri * row_pitch;
            pos[nid] = (x, y, bw);
        }
    }

    let mut canvas = Canvas::new(total_w, canvas_h.max(3));
    let mut extra_edges: Vec<&GraphEdge> = Vec::new();

    // 1. 先绘制相邻层之间的连接线（拐角靠左，给右侧分支水平段留出完整标签空间）
    for e in &graph.edges {
        let (la, lb) = (node_layer[e.from], node_layer[e.to]);
        if la.abs_diff(lb) != 1 {
            extra_edges.push(e);
            continue;
        }
        let forward = (!reverse && lb == la + 1) || (reverse && la == lb + 1);
        let (left_id, right_id, arrow_right) = if la < lb {
            (e.from, e.to, forward)
        } else {
            (e.to, e.from, !forward)
        };
        let (lx, ly, lw) = pos[left_id];
        let (rx, ry, _rw) = pos[right_id];
        let y1 = ly + 1;
        let y2 = ry + 1;
        let x_start = lx + lw;
        let x_end = rx.saturating_sub(1);
        if x_start > x_end {
            extra_edges.push(e);
            continue;
        }
        let line_style = theme::faint();
        let dashed = e.style == EdgeStyle::Dashed;
        let mid_x = (x_start + 1).min(x_end);

        if y1 == y2 {
            canvas.draw_hline(y1, x_start, x_end, dashed, line_style);
        } else {
            if mid_x > x_start {
                canvas.draw_hline(y1, x_start, mid_x, dashed, line_style);
            }
            canvas.merge_line(
                mid_x,
                y1,
                DIR_LEFT | if y2 > y1 { DIR_DOWN } else { DIR_UP },
                line_style,
            );
            let (vy1, vy2) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
            canvas.draw_vline(mid_x, vy1, vy2, dashed, line_style);
            canvas.merge_line(
                mid_x,
                y2,
                DIR_RIGHT | if y2 > y1 { DIR_UP } else { DIR_DOWN },
                line_style,
            );
            if x_end > mid_x {
                canvas.draw_hline(y2, mid_x, x_end, dashed, line_style);
            }
        }

        if e.style != EdgeStyle::Undirected {
            if arrow_right {
                canvas.put_char(x_end, y2, '▶', theme::accent());
            } else {
                canvas.put_char(x_start, y1, '◀', theme::accent());
            }
        }

        if let Some(lbl) = &e.label {
            let span_left = if y1 == y2 { x_start } else { mid_x + 1 };
            let span_w = x_end.saturating_sub(span_left).max(1);
            let short = clip_width(lbl, span_w);
            let label_y = if y1 == y2 {
                y1.saturating_sub(1)
            } else {
                y2.saturating_sub(1)
            };
            canvas.put_centered(span_left, label_y, span_w, &short, theme::thought());
        }
    }

    // 2. 绘制所有节点（覆盖在连接线端点之上，边界干净利落）
    for (nid, node) in graph.nodes.iter().enumerate() {
        let (x, y, bw) = pos[nid];
        draw_node(&mut canvas, node, x, y, bw);
    }

    let mut rows = canvas.into_folded();
    append_extra_edges(&mut rows, graph, &extra_edges, max_width);
    Some(rows)
}

/// 纵向分层布局（适用于 `TD`/`TB`/`BT` 以及窄屏降级）。
fn render_vertical(
    graph: &FlowGraph,
    layers: &[Vec<usize>],
    max_width: usize,
    reverse: bool,
) -> Option<Vec<Folded>> {
    let mut node_layer = vec![0usize; graph.nodes.len()];
    for (li, col) in layers.iter().enumerate() {
        for &idx in col {
            node_layer[idx] = li;
        }
    }

    let gap_x = 3usize;
    let mut pos: Vec<(usize, usize, usize)> = vec![(0, 0, 0); graph.nodes.len()];
    let mut cur_y = 0usize;

    for (li, layer_nodes) in layers.iter().enumerate() {
        let mut sub_rows: Vec<Vec<(usize, usize)>> = Vec::new();
        let mut current: Vec<(usize, usize)> = Vec::new();
        let mut used = 0usize;

        for &nid in layer_nodes {
            let bw = node_box_width(&graph.nodes[nid], max_width.min(34));
            let need = if current.is_empty() { bw } else { gap_x + bw };
            if !current.is_empty() && used + need > max_width {
                sub_rows.push(std::mem::take(&mut current));
                used = 0;
            }
            used += if current.is_empty() { bw } else { gap_x + bw };
            current.push((nid, bw));
        }
        if !current.is_empty() {
            sub_rows.push(current);
        }

        let has_outgoing_label = graph.edges.iter().any(|e| {
            let (la, lb) = (node_layer[e.from], node_layer[e.to]);
            la.min(lb) == li && la.abs_diff(lb) == 1 && e.label.is_some()
        });
        let inter_gap = if has_outgoing_label { 4 } else { 3 };

        for (si, sub) in sub_rows.iter().enumerate() {
            let sub_w: usize = sub.iter().map(|(_, w)| *w).sum::<usize>()
                + gap_x * sub.len().saturating_sub(1);
            let mut x = max_width.saturating_sub(sub_w) / 2;
            for &(nid, bw) in sub {
                pos[nid] = (x, cur_y, bw);
                x += bw + gap_x;
            }
            cur_y += 3 + if si + 1 < sub_rows.len() { 1 } else { inter_gap };
        }
    }

    let total_h = cur_y.max(3);
    let mut canvas = Canvas::new(max_width, total_h);
    let mut extra_edges: Vec<&GraphEdge> = Vec::new();

    // 1. 绘制相邻层之间的垂直与正交分叉连线
    for e in &graph.edges {
        let (la, lb) = (node_layer[e.from], node_layer[e.to]);
        if la.abs_diff(lb) != 1 {
            extra_edges.push(e);
            continue;
        }
        let (top_id, bot_id, arrow_down) = if la < lb {
            (e.from, e.to, !reverse)
        } else {
            (e.to, e.from, reverse)
        };
        let (tx, ty, tw) = pos[top_id];
        let (bx, by, bw) = pos[bot_id];
        let y_start = ty + 3;
        let y_end = by.saturating_sub(1);
        if y_start > y_end {
            extra_edges.push(e);
            continue;
        }
        let cx1 = tx + tw / 2;
        let cx2 = bx + bw / 2;
        let line_style = theme::faint();
        let dashed = e.style == EdgeStyle::Dashed;
        let mid_y = y_start + (y_end - y_start) / 2;

        if cx1 == cx2 {
            canvas.draw_vline(cx1, y_start, y_end, dashed, line_style);
        } else {
            if mid_y > y_start {
                canvas.draw_vline(cx1, y_start, mid_y - 1, dashed, line_style);
            }
            canvas.merge_line(
                cx1,
                mid_y,
                DIR_UP | if cx2 > cx1 { DIR_RIGHT } else { DIR_LEFT },
                line_style,
            );
            let (hx1, hx2) = if cx1 < cx2 {
                (cx1 + 1, cx2.saturating_sub(1))
            } else {
                (cx2 + 1, cx1.saturating_sub(1))
            };
            if hx1 <= hx2 {
                canvas.draw_hline(mid_y, hx1, hx2, dashed, line_style);
            }
            canvas.merge_line(
                cx2,
                mid_y,
                DIR_DOWN | if cx2 > cx1 { DIR_LEFT } else { DIR_RIGHT },
                line_style,
            );
            if y_end > mid_y {
                canvas.draw_vline(cx2, mid_y + 1, y_end, dashed, line_style);
            }
        }

        if e.style != EdgeStyle::Undirected {
            if arrow_down {
                canvas.put_char(cx2, y_end, '▼', theme::accent());
            } else {
                canvas.put_char(cx1, y_start, '▲', theme::accent());
            }
        }

        if let Some(lbl) = &e.label {
            let short = clip_width(lbl, 18);
            let lw = short.width();
            let label_y = if y_end > mid_y { mid_y } else { y_start };
            let anchor_x = cx2.saturating_add(1);
            let draw_x = if anchor_x + lw <= max_width {
                anchor_x
            } else {
                cx2.saturating_sub(lw + 1)
            };
            canvas.put_str(draw_x, label_y, &short, theme::thought());
        }
    }

    // 2. 绘制全部节点
    for (nid, node) in graph.nodes.iter().enumerate() {
        let (x, y, bw) = pos[nid];
        draw_node(&mut canvas, node, x, y, bw);
    }

    let mut rows = canvas.into_folded();
    append_extra_edges(&mut rows, graph, &extra_edges, max_width);
    Some(rows)
}

/// 将跨层边或回环边以紧凑可读的连接说明附在图末，保证零信息丢失。
fn append_extra_edges(
    rows: &mut Vec<Folded>,
    graph: &FlowGraph,
    extra: &[&GraphEdge],
    max_width: usize,
) {
    if extra.is_empty() {
        return;
    }
    rows.push(Folded::default());
    for e in extra {
        let from = &graph.nodes[e.from].label;
        let to = &graph.nodes[e.to].label;
        let arrow = match e.style {
            EdgeStyle::Dashed => "┄▶",
            EdgeStyle::Undirected => "──",
            _ => "─▶",
        };
        let line = match &e.label {
            Some(lbl) => format!("  {from} {arrow} [{lbl}] {to}"),
            None => format!("  {from} {arrow} {to}"),
        };
        let clipped = clip_width(&line, max_width);
        rows.push(Folded {
            spans: vec![ratatui::text::Span::styled(clipped, theme::dim())],
            links: Vec::new(),
            joined: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::inline::plain;

    #[test]
    fn flowchart_lr_renders_boxes_and_arrows() {
        let src = "\
flowchart LR
    A[用户输入] --> B[ACP Client]
    B --> C{响应类型}
    C -->|Markdown| D[框线渲染]
    C -->|ToolCall| E[工具卡片]
";
        let rows = render(src, 96).expect("should render flowchart");
        let joined = rows.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("用户输入"));
        assert!(joined.contains("ACP Client"));
        assert!(joined.contains("◇ 响应类型"));
        assert!(joined.contains("Markdown"));
        assert!(joined.contains('▶'));
    }

    #[test]
    fn flowchart_lr_falls_back_to_vertical_on_narrow_width() {
        let src = "flowchart LR\nA[步骤一] --> B[步骤二] --> C[步骤三] --> D[步骤四]\n";
        let rows = render(src, 30).expect("should degrade to vertical");
        let joined = rows.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(joined.contains('▼'));
        assert!(rows.iter().all(|r| plain(r).width() <= 30));
    }
}
