use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const OUTPUT_DIR: &str = ".ai/graphify-light";
pub const GRAPH_JSON: &str = ".ai/graphify-light/graph.json";
pub const CONFIDENCE_EXTRACTED: &str = "EXTRACTED";
pub const CONFIDENCE_INFERRED: &str = "INFERRED";
pub const CONFIDENCE_AMBIGUOUS: &str = "AMBIGUOUS";

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub file_type: String,
    pub source_file: String,
    pub source_location: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub confidence: String,
    pub source_file: String,
    pub source_location: Option<String>,
    pub confidence_score: f32,
    pub weight: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    #[serde(default = "format_version")]
    pub format_version: u32,
    #[serde(default = "generator")]
    pub generator: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub file_count: usize,
    pub language_counts: BTreeMap<String, usize>,
    pub node_type_counts: BTreeMap<String, usize>,
    pub relation_counts: BTreeMap<String, usize>,
    pub confidence_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Default)]
pub struct GraphBuilder {
    nodes: BTreeMap<String, Node>,
    edge_keys: BTreeSet<(String, String, String, Option<String>)>,
    edges: Vec<Edge>,
}

fn format_version() -> u32 {
    1
}

fn generator() -> String {
    format!("graphify-light {}", env!("CARGO_PKG_VERSION"))
}

impl Node {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        node_type: impl Into<String>,
        language: Option<String>,
        source_file: impl Into<String>,
        source_location: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            file_type: "code".to_string(),
            source_file: source_file.into(),
            source_location,
            node_type: Some(node_type.into()),
            language,
            metadata: BTreeMap::new(),
        }
    }
}

impl Edge {
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        relation: impl Into<String>,
        confidence: impl Into<String>,
        source_file: impl Into<String>,
        source_location: Option<String>,
        context: Option<String>,
    ) -> Self {
        let confidence = confidence.into();
        Self {
            source: source.into(),
            target: target.into(),
            relation: relation.into(),
            confidence_score: confidence_score(&confidence),
            confidence,
            source_file: source_file.into(),
            source_location,
            weight: 1.0,
            context,
            metadata: BTreeMap::new(),
        }
    }
}

impl GraphBuilder {
    pub fn add_node(&mut self, node: Node) {
        self.nodes.entry(node.id.clone()).or_insert(node);
    }

    pub fn add_edge(&mut self, edge: Edge) {
        let key = (
            edge.source.clone(),
            edge.target.clone(),
            edge.relation.clone(),
            edge.source_location.clone(),
        );
        if self.edge_keys.insert(key) {
            self.edges.push(edge);
        }
    }

    pub fn contains_node(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn into_graph(mut self) -> Graph {
        let node_ids: BTreeSet<String> = self.nodes.keys().cloned().collect();
        self.edges
            .retain(|edge| node_ids.contains(&edge.source) && node_ids.contains(&edge.target));
        let mut nodes: Vec<Node> = self.nodes.into_values().collect();
        nodes.sort_by(node_sort_key);
        self.edges.sort_by(edge_sort_key);
        Graph {
            format_version: format_version(),
            generator: generator(),
            nodes,
            edges: self.edges,
        }
    }
}

impl Graph {
    pub fn stats(&self) -> GraphStats {
        let mut language_counts = BTreeMap::new();
        let mut node_type_counts = BTreeMap::new();
        let mut relation_counts = BTreeMap::new();
        let mut confidence_counts = BTreeMap::new();
        let mut files = BTreeSet::new();

        for node in &self.nodes {
            if node.node_type.as_deref() == Some("file") {
                files.insert(node.source_file.clone());
            }
            if let Some(language) = &node.language {
                *language_counts.entry(language.clone()).or_insert(0) += 1;
            }
            let node_type = node
                .node_type
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            *node_type_counts.entry(node_type).or_insert(0) += 1;
        }
        for edge in &self.edges {
            *relation_counts.entry(edge.relation.clone()).or_insert(0) += 1;
            *confidence_counts
                .entry(edge.confidence.clone())
                .or_insert(0) += 1;
        }

        GraphStats {
            node_count: self.nodes.len(),
            edge_count: self.edges.len(),
            file_count: files.len(),
            language_counts,
            node_type_counts,
            relation_counts,
            confidence_counts,
        }
    }
}

pub fn confidence_score(confidence: &str) -> f32 {
    match confidence {
        CONFIDENCE_EXTRACTED => 1.0,
        CONFIDENCE_INFERRED => 0.8,
        CONFIDENCE_AMBIGUOUS => 0.5,
        _ => 1.0,
    }
}

pub fn graph_path(root: &Path) -> PathBuf {
    root.join(GRAPH_JSON)
}

pub fn output_dir(root: &Path) -> PathBuf {
    root.join(OUTPUT_DIR)
}

pub fn read_graph(root: &Path) -> Result<Graph> {
    let path = graph_path(root);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read graph at {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn write_graph(root: &Path, graph: &Graph) -> Result<PathBuf> {
    let dir = output_dir(root);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = graph_path(root);
    let text = serde_json::to_string_pretty(graph)?;
    fs::write(&path, format!("{text}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn repo_relative_path(root: &Path, path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root_absolute = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    absolute
        .strip_prefix(&root_absolute)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn normalize_query_path(path: &str) -> String {
    let p = Path::new(path);
    let normalized = p
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    normalized.trim_start_matches("./").replace('\\', "/")
}

pub fn line_location(line: usize) -> Option<String> {
    Some(format!("L{line}"))
}

pub fn make_id(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .filter(|part| !part.trim().is_empty())
        .map(|part| part.trim())
        .collect::<Vec<_>>()
        .join("_");
    let mut out = String::new();
    let mut last_was_sep = false;
    for ch in joined.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "node".to_string()
    } else {
        trimmed
    }
}

pub fn file_id(rel_path: &str) -> String {
    make_id(&[rel_path])
}

pub fn symbol_id(rel_path: &str, symbol: &str) -> String {
    make_id(&[rel_path, symbol])
}

pub fn import_id(rel_path: &str, import_specifier: &str) -> String {
    make_id(&[rel_path, "import", import_specifier])
}

fn node_sort_key(node: &Node, other: &Node) -> std::cmp::Ordering {
    (
        &node.source_file,
        &node.source_location,
        &node.id,
        &node.label,
    )
        .cmp(&(
            &other.source_file,
            &other.source_location,
            &other.id,
            &other.label,
        ))
}

fn edge_sort_key(edge: &Edge, other: &Edge) -> std::cmp::Ordering {
    (
        &edge.source_file,
        &edge.source_location,
        &edge.source,
        &edge.target,
        &edge.relation,
    )
        .cmp(&(
            &other.source_file,
            &other.source_location,
            &other.source,
            &other.target,
            &other.relation,
        ))
}
