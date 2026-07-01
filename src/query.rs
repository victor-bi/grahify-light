use crate::graph::{normalize_query_path, Graph, Node};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub fn find_symbol(graph: &Graph, name: &str) -> Value {
    let needle = name.to_ascii_lowercase();
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    for node in symbol_nodes(graph) {
        let label = node.label.to_ascii_lowercase();
        if label == needle || node.id.to_ascii_lowercase() == needle {
            exact.push(json!(node));
        } else if label.contains(&needle) || node.id.to_ascii_lowercase().contains(&needle) {
            partial.push(json!(node));
        }
    }
    exact.extend(partial);
    exact.truncate(50);
    json!({
        "query": name,
        "count": exact.len(),
        "matches": exact
    })
}

pub fn get_callers(graph: &Graph, name: &str) -> Value {
    let targets = matching_symbol_ids(graph, name);
    let node_index = node_index(graph);
    let mut results = Vec::new();
    for edge in graph.edges.iter().filter(|edge| {
        matches!(edge.relation.as_str(), "calls" | "indirect_call")
            && targets.contains(&edge.target)
    }) {
        if let Some(caller) = node_index.get(&edge.source) {
            results.push(json!({
                "caller": caller,
                "target": node_index.get(&edge.target),
                "edge": edge
            }));
        }
    }
    json!({
        "query": name,
        "count": results.len(),
        "callers": results
    })
}

pub fn get_callees(graph: &Graph, name: &str) -> Value {
    let callers = matching_symbol_ids(graph, name);
    let node_index = node_index(graph);
    let mut results = Vec::new();
    for edge in graph.edges.iter().filter(|edge| {
        matches!(edge.relation.as_str(), "calls" | "indirect_call")
            && callers.contains(&edge.source)
    }) {
        if let Some(callee) = node_index.get(&edge.target) {
            results.push(json!({
                "caller": node_index.get(&edge.source),
                "callee": callee,
                "edge": edge
            }));
        }
    }
    json!({
        "query": name,
        "count": results.len(),
        "callees": results
    })
}

pub fn get_file_symbols(graph: &Graph, path: &str) -> Value {
    let path = normalize_query_path(path);
    let symbols = graph
        .nodes
        .iter()
        .filter(|node| node.source_file == path && node.node_type.as_deref() != Some("file"))
        .map(|node| json!(node))
        .collect::<Vec<_>>();
    json!({
        "path": path,
        "count": symbols.len(),
        "symbols": symbols
    })
}

pub fn search_nodes(graph: &Graph, text: &str) -> Value {
    let needle = text.to_ascii_lowercase();
    let mut matches = graph
        .nodes
        .iter()
        .filter(|node| {
            node.label.to_ascii_lowercase().contains(&needle)
                || node.id.to_ascii_lowercase().contains(&needle)
                || node.source_file.to_ascii_lowercase().contains(&needle)
        })
        .map(|node| json!(node))
        .collect::<Vec<_>>();
    matches.truncate(100);
    json!({
        "query": text,
        "count": matches.len(),
        "matches": matches
    })
}

pub fn get_related_files(graph: &Graph, path: &str) -> Value {
    let path = normalize_query_path(path);
    let node_index = node_index(graph);
    let file_ids = graph
        .nodes
        .iter()
        .filter(|node| node.source_file == path)
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    let mut related: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for edge in &graph.edges {
        let source_in_file = file_ids.contains(&edge.source);
        let target_in_file = file_ids.contains(&edge.target);
        if !source_in_file && !target_in_file {
            continue;
        }
        let other_id = if source_in_file {
            &edge.target
        } else {
            &edge.source
        };
        if let Some(other) = node_index.get(other_id) {
            if other.source_file != path && !other.source_file.is_empty() {
                related
                    .entry(other.source_file.clone())
                    .or_default()
                    .push(json!({
                        "relation": edge.relation,
                        "direction": if source_in_file { "outgoing" } else { "incoming" },
                        "edge": edge,
                        "node": other
                    }));
            }
        }
    }
    let files = related
        .into_iter()
        .map(|(file, reasons)| json!({"path": file, "reasons": reasons}))
        .collect::<Vec<_>>();
    json!({
        "path": path,
        "count": files.len(),
        "files": files
    })
}

pub fn get_imports(graph: &Graph, path: &str) -> Value {
    let path = normalize_query_path(path);
    let file_node_ids = graph
        .nodes
        .iter()
        .filter(|node| node.source_file == path && node.node_type.as_deref() == Some("file"))
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    let node_index = node_index(graph);
    let imports = graph
        .edges
        .iter()
        .filter(|edge| {
            file_node_ids.contains(&edge.source)
                && matches!(edge.relation.as_str(), "imports" | "imports_from")
        })
        .map(|edge| {
            json!({
                "target": node_index.get(&edge.target),
                "edge": edge
            })
        })
        .collect::<Vec<_>>();
    json!({
        "path": path,
        "count": imports.len(),
        "imports": imports
    })
}

pub fn get_exports(graph: &Graph, path: &str) -> Value {
    let path = normalize_query_path(path);
    let exports = graph
        .nodes
        .iter()
        .filter(|node| {
            node.source_file == path
                && matches!(
                    node.node_type.as_deref(),
                    Some("function")
                        | Some("method")
                        | Some("constructor")
                        | Some("class")
                        | Some("struct")
                        | Some("enum")
                        | Some("trait")
                        | Some("interface")
                        | Some("module")
                        | Some("symbol")
                )
        })
        .map(|node| json!(node))
        .collect::<Vec<_>>();
    json!({
        "path": path,
        "count": exports.len(),
        "exports": exports
    })
}

pub fn get_graph_stats(graph: &Graph) -> Value {
    json!(graph.stats())
}

fn symbol_nodes(graph: &Graph) -> impl Iterator<Item = &Node> {
    graph.nodes.iter().filter(|node| {
        node.node_type.as_deref() != Some("file") && node.node_type.as_deref() != Some("import")
    })
}

fn matching_symbol_ids(graph: &Graph, name: &str) -> BTreeSet<String> {
    let needle = name.to_ascii_lowercase();
    let exact = symbol_nodes(graph)
        .filter(|node| node.label.to_ascii_lowercase() == needle)
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    if !exact.is_empty() {
        return exact;
    }
    symbol_nodes(graph)
        .filter(|node| node.label.to_ascii_lowercase().contains(&needle))
        .map(|node| node.id.clone())
        .collect()
}

fn node_index(graph: &Graph) -> BTreeMap<String, &Node> {
    graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect()
}
