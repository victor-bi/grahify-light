use crate::config::{EffectiveExtraction, GraphifyConfig};
use crate::detect::{collect_indexable_files, DetectedFile, ExtractorKind, SupportedLanguage};
use crate::graph::{
    file_id, import_id, line_location, make_id, symbol_id, Edge, Graph, GraphBuilder, Node,
    CONFIDENCE_EXTRACTED, CONFIDENCE_INFERRED,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use tree_sitter::{Language, Node as TsNode, Parser};

#[derive(Debug, Clone)]
struct RawCall {
    caller_id: String,
    callee: String,
    source_file: String,
    source_location: Option<String>,
}

#[derive(Debug, Clone)]
struct ImportRecord {
    file_id: String,
    specifier: String,
    source_file: String,
    source_location: Option<String>,
}

#[derive(Debug, Clone)]
struct DefinitionRecord {
    id: String,
    label: String,
    file_id: String,
    source_file: String,
}

#[derive(Debug, Default)]
struct ExtractionSidecars {
    raw_calls: Vec<RawCall>,
    imports: Vec<ImportRecord>,
    definitions: Vec<DefinitionRecord>,
}

#[derive(Debug)]
struct WalkState<'a> {
    file: &'a DetectedFile,
    file_id: String,
    current_callable: Option<String>,
    class_stack: Vec<String>,
}

pub fn build_graph(root: &Path) -> Result<Graph> {
    let config = GraphifyConfig::load(root)?;
    build_graph_with_config(root, &config)
}

pub fn build_graph_with_config(root: &Path, config: &GraphifyConfig) -> Result<Graph> {
    let extraction = config.effective_extraction();
    let files = collect_indexable_files(root)?;
    let file_lookup = build_file_lookup(&files);
    let mut builder = GraphBuilder::default();
    let mut sidecars = ExtractionSidecars::default();

    for file in &files {
        let file_result = extract_detected_file(file, &file_lookup, &extraction)
            .with_context(|| format!("failed to extract {}", file.rel_path))?;
        for node in file_result.0 {
            builder.add_node(node);
        }
        for edge in file_result.1 {
            builder.add_edge(edge);
        }
        sidecars.raw_calls.extend(file_result.2.raw_calls);
        sidecars.imports.extend(file_result.2.imports);
        sidecars.definitions.extend(file_result.2.definitions);
    }

    resolve_imports(&mut builder, &sidecars.imports, &file_lookup);
    resolve_calls(&mut builder, &sidecars);

    Ok(builder.into_graph())
}

fn extract_detected_file(
    file: &DetectedFile,
    file_lookup: &BTreeMap<String, String>,
    extraction: &EffectiveExtraction,
) -> Result<(Vec<Node>, Vec<Edge>, ExtractionSidecars)> {
    let bytes = std::fs::read(&file.path)?;
    let mut degraded_reason = extraction.degraded_reason.clone();
    let result = match file.extractor {
        ExtractorKind::TreeSitter(language) => match extract_tree_sitter(
            file,
            file.supported_language.unwrap_or(language),
            &bytes,
            file_lookup,
        ) {
            Ok(result) => result,
            Err(_) => {
                degraded_reason =
                    merge_degraded_reason(degraded_reason, "tree_sitter_failed_heuristic_fallback");
                extract_heuristic(file, &bytes, file_lookup)
            }
        },
        ExtractorKind::HeuristicCode => extract_heuristic(file, &bytes, file_lookup),
        ExtractorKind::Terraform => extract_terraform(file, &bytes),
        ExtractorKind::Ansible => extract_ansible(file, &bytes),
        ExtractorKind::JsonConfig => extract_json_config(file, &bytes),
        ExtractorKind::TomlConfig => extract_toml_config(file, &bytes),
        ExtractorKind::YamlConfig => extract_yaml_config(file, &bytes),
        ExtractorKind::PackageManifest => extract_package_manifest(file, &bytes),
        ExtractorKind::McpConfig => extract_mcp_config(file, &bytes),
        ExtractorKind::Markdown => extract_markdown(file, &bytes, file_lookup),
        ExtractorKind::Text => extract_text_document(file, &bytes),
        ExtractorKind::Html => extract_html_document(file, &bytes),
        ExtractorKind::Pdf => extract_pdf(file, &bytes),
        ExtractorKind::Office => extract_office(file, &bytes),
        ExtractorKind::Image => extract_image(file, &bytes),
        ExtractorKind::AudioVideo => extract_audio_video(file, &bytes),
        ExtractorKind::Archive => extract_archive(file, &bytes),
        ExtractorKind::ResourceMetadata => extract_resource_metadata(file, &bytes),
    };
    let (mut nodes, mut edges, sidecars) = result;
    apply_extraction_metadata(
        file,
        extraction,
        degraded_reason.as_deref(),
        &mut nodes,
        &mut edges,
    );
    Ok((nodes, edges, sidecars))
}

fn merge_degraded_reason(current: Option<String>, next: &str) -> Option<String> {
    match current {
        Some(value) if value == next || value.contains(next) => Some(value),
        Some(value) => Some(format!("{value};{next}")),
        None => Some(next.to_string()),
    }
}

fn apply_extraction_metadata(
    file: &DetectedFile,
    extraction: &EffectiveExtraction,
    degraded_reason: Option<&str>,
    nodes: &mut [Node],
    edges: &mut [Edge],
) {
    for node in nodes {
        node.metadata.insert(
            "extraction_mode".to_string(),
            json!(extraction.actual_mode.as_str()),
        );
        if extraction.requested_mode != extraction.actual_mode {
            node.metadata.insert(
                "requested_extraction_mode".to_string(),
                json!(extraction.requested_mode.as_str()),
            );
        }
        node.metadata
            .insert("extractor".to_string(), json!(file.extractor.name()));
        node.metadata.insert(
            "file_category".to_string(),
            json!(file.file_category.as_str()),
        );
        if let Some(reason) = degraded_reason {
            node.metadata
                .insert("degraded_reason".to_string(), json!(reason));
        }
    }
    for edge in edges {
        edge.metadata.insert(
            "extraction_mode".to_string(),
            json!(extraction.actual_mode.as_str()),
        );
        edge.metadata
            .insert("extractor".to_string(), json!(file.extractor.name()));
        if let Some(reason) = degraded_reason {
            edge.metadata
                .insert("degraded_reason".to_string(), json!(reason));
        }
    }
}

fn extract_tree_sitter(
    file: &DetectedFile,
    supported_language: SupportedLanguage,
    source: &[u8],
    file_lookup: &BTreeMap<String, String>,
) -> Result<(Vec<Node>, Vec<Edge>, ExtractionSidecars)> {
    let mut parser = Parser::new();
    let language = tree_sitter_language(supported_language);
    parser.set_language(&language)?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parser returned no tree")?;

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut sidecars = ExtractionSidecars::default();
    add_file_node(file, &mut nodes);

    let state = WalkState {
        file,
        file_id: file_id(&file.rel_path),
        current_callable: None,
        class_stack: Vec::new(),
    };
    walk_tree(
        tree.root_node(),
        source,
        supported_language,
        file_lookup,
        state,
        &mut nodes,
        &mut edges,
        &mut sidecars,
    );
    Ok((nodes, edges, sidecars))
}

fn extract_resource_metadata(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let mut nodes = Vec::new();
    add_file_node(file, &mut nodes);
    if let Some(node) = nodes.first_mut() {
        node.metadata.insert(
            "content_kind".to_string(),
            json!("unknown_binary_or_resource"),
        );
        node.metadata
            .insert("magic".to_string(), json!(magic_hex(bytes)));
    }
    (nodes, Vec::new(), ExtractionSidecars::default())
}

fn extract_text_document(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);
    if let Some(node) = nodes.first_mut() {
        node.metadata
            .insert("word_count".to_string(), json!(count_words(&text)));
    }

    let mut last_heading: Option<String> = None;
    for (idx, line) in text.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() > 120 {
            continue;
        }
        let is_heading = trimmed.ends_with(':')
            || (line_number < 80
                && trimmed.chars().all(|ch| {
                    ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() || "-_./".contains(ch)
                })
                && trimmed.split_whitespace().count() <= 10);
        if !is_heading || last_heading.as_deref() == Some(trimmed) {
            continue;
        }
        last_heading = Some(trimmed.to_string());
        let node_id = make_id(&[&file.rel_path, "section", trimmed]);
        nodes.push(child_node(
            file,
            node_id.clone(),
            trimmed,
            "section",
            line_number,
        ));
        edges.push(contains_edge(file, &node_id, line_number, "section"));
    }

    (nodes, edges, ExtractionSidecars::default())
}

fn extract_html_document(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);

    let title_regex = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid html regex");
    if let Some(title) = title_regex
        .captures(&text)
        .and_then(|caps| caps.get(1).map(|m| clean_html_text(m.as_str())))
        .filter(|title| !title.is_empty())
    {
        if let Some(node) = nodes.first_mut() {
            node.metadata.insert("title".to_string(), json!(title));
        }
    }

    let heading_regex =
        Regex::new(r"(?is)<h([1-6])[^>]*>(.*?)</h[1-6]>").expect("valid html regex");
    for caps in heading_regex.captures_iter(&text).take(200) {
        let Some(raw) = caps.get(2) else {
            continue;
        };
        let label = clean_html_text(raw.as_str());
        if label.is_empty() {
            continue;
        }
        let line = line_number_for_offset(&text, raw.start());
        let node_id = make_id(&[&file.rel_path, "heading", &label, &line.to_string()]);
        let mut node = child_node(file, node_id.clone(), &label, "heading", line);
        node.metadata.insert(
            "level".to_string(),
            json!(caps.get(1).map(|m| m.as_str()).unwrap_or("")),
        );
        nodes.push(node);
        edges.push(contains_edge(file, &node_id, line, "heading"));
    }

    (nodes, edges, ExtractionSidecars::default())
}

fn extract_markdown(
    file: &DetectedFile,
    bytes: &[u8],
    file_lookup: &BTreeMap<String, String>,
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);

    let file_node_id = file_id(&file.rel_path);
    let heading_regex = Regex::new(r"^(#{1,6})\s+(.+?)\s*$").expect("valid markdown regex");
    let inline_link_regex = Regex::new(r"!?\[[^\]]*\]\(([^)]+)\)").expect("valid markdown regex");
    let wiki_link_regex = Regex::new(r"\[\[([^\]|#]+)").expect("valid markdown regex");
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut linked_targets = BTreeSet::new();
    let mut in_code_block = false;

    for (idx, line) in text.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        for raw in inline_link_regex
            .captures_iter(line)
            .filter_map(|caps| caps.get(1).map(|m| m.as_str()))
            .chain(
                wiki_link_regex
                    .captures_iter(line)
                    .filter_map(|caps| caps.get(1).map(|m| m.as_str())),
            )
        {
            if let Some(target_id) = resolve_document_link(&file.rel_path, raw, file_lookup) {
                if target_id != file_node_id && linked_targets.insert(target_id.clone()) {
                    edges.push(Edge::new(
                        file_node_id.clone(),
                        target_id,
                        "references",
                        CONFIDENCE_EXTRACTED,
                        file.rel_path.clone(),
                        line_location(line_number),
                        Some("link".to_string()),
                    ));
                }
            }
        }

        let Some(caps) = heading_regex.captures(line) else {
            continue;
        };
        let level = caps.get(1).map(|m| m.as_str().len()).unwrap_or(1);
        let title = caps
            .get(2)
            .map(|m| m.as_str().trim().trim_matches('#').trim())
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        while heading_stack
            .last()
            .map(|(existing, _)| *existing >= level)
            .unwrap_or(false)
        {
            heading_stack.pop();
        }
        let node_id = make_id(&[&file.rel_path, "heading", title, &line_number.to_string()]);
        let mut node = child_node(file, node_id.clone(), title, "heading", line_number);
        node.metadata.insert("level".to_string(), json!(level));
        nodes.push(node);
        let parent = heading_stack
            .last()
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| file_node_id.clone());
        edges.push(Edge::new(
            parent,
            node_id.clone(),
            "contains",
            CONFIDENCE_EXTRACTED,
            file.rel_path.clone(),
            line_location(line_number),
            Some("heading".to_string()),
        ));
        heading_stack.push((level, node_id));
    }

    (nodes, edges, ExtractionSidecars::default())
}

fn extract_json_config(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let parsed = serde_json::from_str::<Value>(&text)
        .or_else(|_| serde_json::from_str::<Value>(&strip_jsonc(&text)));
    let Ok(value) = parsed else {
        return extract_resource_metadata(file, bytes);
    };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);
    if !looks_like_config_json(file, &value) {
        if let Some(node) = nodes.first_mut() {
            node.metadata
                .insert("skipped_structure".to_string(), json!("data_json"));
        }
        return (nodes, edges, ExtractionSidecars::default());
    }

    if let Value::Object(map) = value {
        add_object_keys(
            file,
            &mut nodes,
            &mut edges,
            &file_id(&file.rel_path),
            None,
            &map,
            0,
        );
    }
    (nodes, edges, ExtractionSidecars::default())
}

fn extract_toml_config(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let parsed = toml::from_str::<toml::Value>(&text);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);
    let Ok(value) = parsed else {
        return (nodes, edges, ExtractionSidecars::default());
    };
    if let Some(table) = value.as_table() {
        for (idx, key) in table.keys().take(200).enumerate() {
            let node_id = make_id(&[&file.rel_path, "toml", key]);
            nodes.push(child_node(
                file,
                node_id.clone(),
                key,
                "config_key",
                idx + 1,
            ));
            edges.push(contains_edge(file, &node_id, idx + 1, "config"));
        }
    }
    (nodes, edges, ExtractionSidecars::default())
}

fn extract_yaml_config(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);

    let docs = serde_yaml::Deserializer::from_str(&text)
        .filter_map(|doc| serde_yaml::Value::deserialize(doc).ok())
        .collect::<Vec<_>>();
    if docs.is_empty() {
        return (nodes, edges, ExtractionSidecars::default());
    }

    if docs.iter().any(is_kubernetes_doc) {
        extract_kubernetes_docs(file, &docs, &mut nodes, &mut edges);
    } else if let Some(serde_yaml::Value::Mapping(map)) = docs.first() {
        for (idx, key) in map.keys().filter_map(yaml_key_string).take(200).enumerate() {
            let node_id = make_id(&[&file.rel_path, "yaml", &key]);
            nodes.push(child_node(
                file,
                node_id.clone(),
                &key,
                "config_key",
                idx + 1,
            ));
            edges.push(contains_edge(file, &node_id, idx + 1, "config"));
        }
    }
    (nodes, edges, ExtractionSidecars::default())
}

fn extract_ansible(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);

    let docs = serde_yaml::Deserializer::from_str(&text)
        .filter_map(|doc| serde_yaml::Value::deserialize(doc).ok())
        .collect::<Vec<_>>();
    if docs.is_empty() {
        return (nodes, edges, ExtractionSidecars::default());
    }

    let role_context = ansible_role_context(&file.rel_path);
    let owner_id = if let Some((role, section)) = &role_context {
        let role_id = add_ansible_role_node(file, &mut nodes, role, 1);
        edges.push(contains_edge(file, &role_id, 1, "ansible_role"));
        let component_id = add_ansible_role_component_node(file, &mut nodes, role, section, 1);
        edges.push(Edge::new(
            role_id.clone(),
            component_id.clone(),
            "contains",
            CONFIDENCE_EXTRACTED,
            file.rel_path.clone(),
            line_location(1),
            Some("ansible_role_component".to_string()),
        ));
        Some((component_id, section.clone(), role_id))
    } else {
        None
    };

    for (doc_idx, doc) in docs.iter().enumerate() {
        let line = doc_idx + 1;
        match doc {
            serde_yaml::Value::Sequence(items) => {
                if let Some((component_id, section, _role_id)) = &owner_id {
                    if matches!(section.as_str(), "tasks" | "handlers") {
                        let task_kind = if section == "handlers" {
                            "handler"
                        } else {
                            "task"
                        };
                        add_ansible_tasks(
                            file,
                            &mut nodes,
                            &mut edges,
                            component_id,
                            items,
                            task_kind,
                            line,
                        );
                        continue;
                    }
                }
                add_ansible_playbook_items(file, &mut nodes, &mut edges, items, line);
            }
            serde_yaml::Value::Mapping(map) => {
                if let Some((component_id, section, role_id)) = &owner_id {
                    match section.as_str() {
                        "meta" => add_ansible_role_dependencies(
                            file, &mut nodes, &mut edges, role_id, map, line,
                        ),
                        "defaults" | "vars" => add_ansible_variables(
                            file,
                            &mut nodes,
                            &mut edges,
                            component_id,
                            map,
                            line,
                        ),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    (nodes, edges, ExtractionSidecars::default())
}

fn add_ansible_playbook_items(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    items: &[serde_yaml::Value],
    line: usize,
) {
    for (idx, item) in items.iter().enumerate() {
        let item_line = line + idx;
        let serde_yaml::Value::Mapping(map) = item else {
            continue;
        };
        if let Some(target) = yaml_map_get(map, "import_playbook").and_then(yaml_scalar_string) {
            let target_id = add_ansible_playbook_ref_node(file, nodes, &target, item_line);
            edges.push(Edge::new(
                file_id(&file.rel_path),
                target_id,
                "imports",
                CONFIDENCE_EXTRACTED,
                file.rel_path.clone(),
                line_location(item_line),
                Some("ansible_import_playbook".to_string()),
            ));
            continue;
        }

        let label = yaml_map_get(map, "name")
            .and_then(yaml_scalar_string)
            .or_else(|| yaml_map_get(map, "hosts").and_then(yaml_scalar_string))
            .unwrap_or_else(|| format!("play {}", idx + 1));
        let play_id = make_id(&[
            &file.rel_path,
            "ansible_play",
            &(idx + 1).to_string(),
            &label,
        ]);
        let mut play_node = child_node(file, play_id.clone(), &label, "ansible_play", item_line);
        if let Some(hosts) = yaml_map_get(map, "hosts").and_then(yaml_scalar_string) {
            play_node.metadata.insert("hosts".to_string(), json!(hosts));
        }
        nodes.push(play_node);
        edges.push(contains_edge(file, &play_id, item_line, "ansible_play"));

        if let Some(roles) = yaml_map_get(map, "roles").and_then(serde_yaml::Value::as_sequence) {
            add_ansible_role_uses(
                file,
                nodes,
                edges,
                &play_id,
                roles,
                item_line,
                "ansible_roles",
            );
        }
        for key in ["pre_tasks", "tasks", "post_tasks"] {
            if let Some(tasks) = yaml_map_get(map, key).and_then(serde_yaml::Value::as_sequence) {
                add_ansible_tasks(file, nodes, edges, &play_id, tasks, "task", item_line);
            }
        }
        if let Some(handlers) =
            yaml_map_get(map, "handlers").and_then(serde_yaml::Value::as_sequence)
        {
            add_ansible_tasks(file, nodes, edges, &play_id, handlers, "handler", item_line);
        }
    }
}

fn add_ansible_tasks(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    owner_id: &str,
    tasks: &[serde_yaml::Value],
    task_kind: &str,
    line: usize,
) {
    for (idx, task) in tasks.iter().enumerate() {
        let task_line = line + idx;
        let serde_yaml::Value::Mapping(map) = task else {
            continue;
        };
        let label = yaml_map_get(map, "name")
            .and_then(yaml_scalar_string)
            .unwrap_or_else(|| format!("{task_kind} {}", idx + 1));
        let node_type = if task_kind == "handler" {
            "ansible_handler"
        } else {
            "ansible_task"
        };
        let task_id = make_id(&[
            &file.rel_path,
            "ansible",
            owner_id,
            task_kind,
            &(idx + 1).to_string(),
            &label,
        ]);
        let mut task_node = child_node(file, task_id.clone(), &label, node_type, task_line);
        if let Some(action) = ansible_task_action(map) {
            task_node
                .metadata
                .insert("action".to_string(), json!(action));
        }
        nodes.push(task_node);
        edges.push(Edge::new(
            owner_id.to_string(),
            task_id.clone(),
            "contains",
            CONFIDENCE_EXTRACTED,
            file.rel_path.clone(),
            line_location(task_line),
            Some(format!("ansible_{task_kind}")),
        ));

        add_ansible_task_relationships(file, nodes, edges, &task_id, map, task_line);

        for nested_key in ["block", "rescue", "always"] {
            if let Some(nested) =
                yaml_map_get(map, nested_key).and_then(serde_yaml::Value::as_sequence)
            {
                add_ansible_tasks(file, nodes, edges, &task_id, nested, "task", task_line);
            }
        }
    }
}

fn add_ansible_task_relationships(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    task_id: &str,
    map: &serde_yaml::Mapping,
    line: usize,
) {
    for key in ["include_tasks", "ansible.builtin.include_tasks"] {
        if let Some(target) = ansible_task_file_arg(yaml_map_get(map, key)) {
            add_ansible_task_file_edge(file, nodes, edges, task_id, &target, "includes", line);
        }
    }
    for key in ["import_tasks", "ansible.builtin.import_tasks"] {
        if let Some(target) = ansible_task_file_arg(yaml_map_get(map, key)) {
            add_ansible_task_file_edge(file, nodes, edges, task_id, &target, "imports", line);
        }
    }
    for key in ["include_role", "ansible.builtin.include_role"] {
        if let Some(role) = ansible_role_arg(yaml_map_get(map, key)) {
            add_ansible_role_edge(file, nodes, edges, task_id, &role, "uses_role", line, key);
        }
    }
    for key in ["import_role", "ansible.builtin.import_role"] {
        if let Some(role) = ansible_role_arg(yaml_map_get(map, key)) {
            add_ansible_role_edge(file, nodes, edges, task_id, &role, "uses_role", line, key);
        }
    }
    if let Some(notify) = yaml_map_get(map, "notify") {
        for handler in yaml_string_values(notify) {
            let handler_id = add_ansible_handler_ref_node(file, nodes, &handler, line);
            edges.push(Edge::new(
                task_id.to_string(),
                handler_id,
                "notifies",
                CONFIDENCE_EXTRACTED,
                file.rel_path.clone(),
                line_location(line),
                Some("ansible_notify".to_string()),
            ));
        }
    }
    if let Some(listen) = yaml_map_get(map, "listen") {
        for topic in yaml_string_values(listen) {
            let topic_id = add_ansible_handler_topic_node(file, nodes, &topic, line);
            edges.push(Edge::new(
                task_id.to_string(),
                topic_id,
                "listens_to",
                CONFIDENCE_EXTRACTED,
                file.rel_path.clone(),
                line_location(line),
                Some("ansible_handler_listen".to_string()),
            ));
        }
    }
}

fn add_ansible_role_uses(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    owner_id: &str,
    roles: &[serde_yaml::Value],
    line: usize,
    context: &str,
) {
    for (idx, role_value) in roles.iter().enumerate() {
        if let Some(role) = ansible_role_arg(Some(role_value)) {
            add_ansible_role_edge(
                file,
                nodes,
                edges,
                owner_id,
                &role,
                "uses_role",
                line + idx,
                context,
            );
        }
    }
}

fn add_ansible_role_dependencies(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    role_id: &str,
    map: &serde_yaml::Mapping,
    line: usize,
) {
    let Some(dependencies) =
        yaml_map_get(map, "dependencies").and_then(serde_yaml::Value::as_sequence)
    else {
        return;
    };
    for (idx, dependency) in dependencies.iter().enumerate() {
        if let Some(role) = ansible_role_arg(Some(dependency)) {
            add_ansible_role_edge(
                file,
                nodes,
                edges,
                role_id,
                &role,
                "depends_on",
                line + idx,
                "ansible_role_dependency",
            );
        }
    }
}

fn add_ansible_variables(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    owner_id: &str,
    map: &serde_yaml::Mapping,
    line: usize,
) {
    for (idx, key) in map.keys().filter_map(yaml_key_string).take(200).enumerate() {
        let var_id = make_id(&[&file.rel_path, "ansible_var", &key]);
        let mut var_node = child_node(file, var_id.clone(), &key, "ansible_variable", line + idx);
        var_node.metadata.insert("name".to_string(), json!(key));
        nodes.push(var_node);
        edges.push(Edge::new(
            owner_id.to_string(),
            var_id,
            "contains",
            CONFIDENCE_EXTRACTED,
            file.rel_path.clone(),
            line_location(line + idx),
            Some("ansible_variable".to_string()),
        ));
    }
}

fn add_ansible_role_edge(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    source_id: &str,
    role: &str,
    relation: &str,
    line: usize,
    context: &str,
) {
    let role_id = add_ansible_role_node(file, nodes, role, line);
    edges.push(Edge::new(
        source_id.to_string(),
        role_id,
        relation,
        CONFIDENCE_EXTRACTED,
        file.rel_path.clone(),
        line_location(line),
        Some(context.to_string()),
    ));
}

fn add_ansible_task_file_edge(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    source_id: &str,
    target: &str,
    relation: &str,
    line: usize,
) {
    let target_path = resolve_ansible_relative_path(&file.rel_path, target);
    let target_id = add_ansible_task_file_node(file, nodes, &target_path, line);
    edges.push(Edge::new(
        source_id.to_string(),
        target_id,
        relation,
        CONFIDENCE_EXTRACTED,
        file.rel_path.clone(),
        line_location(line),
        Some("ansible_task_file".to_string()),
    ));
}

fn add_ansible_role_node(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    role: &str,
    line: usize,
) -> String {
    let role_id = make_id(&["ansible", "role", role]);
    let mut node = child_node(file, role_id.clone(), role, "ansible_role", line);
    node.metadata.insert("role".to_string(), json!(role));
    nodes.push(node);
    role_id
}

fn add_ansible_role_component_node(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    role: &str,
    section: &str,
    line: usize,
) -> String {
    let node_type = match section {
        "tasks" => "ansible_task_file",
        "handlers" => "ansible_handler_file",
        "defaults" | "vars" => "ansible_vars_file",
        "meta" => "ansible_role_meta",
        _ => "ansible_role_component",
    };
    let component_id = make_id(&["ansible", "role", role, section, &file.rel_path]);
    let mut node = child_node(
        file,
        component_id.clone(),
        format!("{role}/{section}"),
        node_type,
        line,
    );
    node.metadata.insert("role".to_string(), json!(role));
    node.metadata
        .insert("section".to_string(), json!(section.to_string()));
    nodes.push(node);
    component_id
}

fn add_ansible_task_file_node(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    path: &str,
    line: usize,
) -> String {
    let node_id = make_id(&["ansible", "task_file", path]);
    nodes.push(child_node(
        file,
        node_id.clone(),
        path,
        "ansible_task_file",
        line,
    ));
    node_id
}

fn add_ansible_playbook_ref_node(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    path: &str,
    line: usize,
) -> String {
    let path = resolve_ansible_relative_path(&file.rel_path, path);
    let node_id = make_id(&["ansible", "playbook", &path]);
    nodes.push(child_node(
        file,
        node_id.clone(),
        path,
        "ansible_playbook",
        line,
    ));
    node_id
}

fn add_ansible_handler_ref_node(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    handler: &str,
    line: usize,
) -> String {
    let handler_id = make_id(&["ansible", "handler", handler]);
    nodes.push(child_node(
        file,
        handler_id.clone(),
        handler,
        "ansible_handler",
        line,
    ));
    handler_id
}

fn add_ansible_handler_topic_node(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    topic: &str,
    line: usize,
) -> String {
    let topic_id = make_id(&["ansible", "handler_topic", topic]);
    nodes.push(child_node(
        file,
        topic_id.clone(),
        topic,
        "ansible_handler_topic",
        line,
    ));
    topic_id
}

fn ansible_role_context(rel_path: &str) -> Option<(String, String)> {
    let parts = rel_path.split('/').collect::<Vec<_>>();
    for window in parts.windows(3) {
        if window[0] == "roles"
            && matches!(
                window[2],
                "tasks" | "handlers" | "defaults" | "vars" | "meta"
            )
        {
            return Some((window[1].to_string(), window[2].to_string()));
        }
    }
    None
}

fn ansible_task_action(map: &serde_yaml::Mapping) -> Option<String> {
    map.keys()
        .filter_map(yaml_key_string)
        .find(|key| !is_ansible_task_keyword(key))
}

fn is_ansible_task_keyword(key: &str) -> bool {
    matches!(
        key,
        "name"
            | "when"
            | "tags"
            | "vars"
            | "register"
            | "notify"
            | "listen"
            | "become"
            | "become_user"
            | "delegate_to"
            | "with_items"
            | "loop"
            | "loop_control"
            | "changed_when"
            | "failed_when"
            | "ignore_errors"
            | "check_mode"
            | "args"
            | "environment"
            | "block"
            | "rescue"
            | "always"
    )
}

fn ansible_task_file_arg(value: Option<&serde_yaml::Value>) -> Option<String> {
    match value? {
        serde_yaml::Value::String(value) => clean_ansible_ref(value),
        serde_yaml::Value::Mapping(map) => yaml_map_get(map, "file").and_then(yaml_scalar_string),
        _ => None,
    }
}

fn ansible_role_arg(value: Option<&serde_yaml::Value>) -> Option<String> {
    match value? {
        serde_yaml::Value::String(value) => clean_ansible_ref(value),
        serde_yaml::Value::Mapping(map) => yaml_map_get(map, "role")
            .or_else(|| yaml_map_get(map, "name"))
            .and_then(yaml_scalar_string),
        _ => None,
    }
}

fn clean_ansible_ref(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("{{") || trimmed.contains("{%") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn yaml_scalar_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(value) => clean_ansible_ref(value),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn yaml_string_values(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .filter_map(yaml_scalar_string)
            .collect::<Vec<_>>(),
        _ => yaml_scalar_string(value).into_iter().collect(),
    }
}

fn yaml_map_get<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a serde_yaml::Value> {
    map.get(serde_yaml::Value::String(key.to_string()))
}

fn resolve_ansible_relative_path(current_file: &str, target: &str) -> String {
    if target.starts_with('/') {
        return normalize_rel_path(target.trim_start_matches('/'));
    }
    let base = Path::new(current_file)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    normalize_rel_path(&base.join(target).to_string_lossy())
}

fn normalize_rel_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in Path::new(path).components() {
        let value = part.as_os_str().to_string_lossy();
        match value.as_ref() {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(value.to_string()),
        }
    }
    parts.join("/")
}

fn extract_terraform(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);

    let block_regex = Regex::new(
        r#"^\s*(resource|data|module|variable|output|provider)\s+"([^"]+)"(?:\s+"([^"]+)")?"#,
    )
    .expect("valid terraform regex");
    let local_regex =
        Regex::new(r#"^\s*([A-Za-z_][A-Za-z0-9_-]*)\s*="#).expect("valid terraform regex");
    let scope = Path::new(&file.rel_path)
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("root");
    let mut address_to_id = BTreeMap::new();
    let mut current_owner: Option<String> = None;
    let mut brace_depth: i32 = 0;
    let mut in_locals = false;

    for (idx, line) in text.lines().enumerate() {
        let line_number = idx + 1;
        if let Some(caps) = block_regex.captures(line) {
            let kind = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let first = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            let second = caps.get(3).map(|m| m.as_str());
            let address = match (kind, second) {
                ("resource", Some(name)) => format!("{first}.{name}"),
                ("data", Some(name)) => format!("data.{first}.{name}"),
                ("module", _) => format!("module.{first}"),
                ("variable", _) => format!("var.{first}"),
                ("output", _) => format!("output.{first}"),
                ("provider", _) => format!("provider.{first}"),
                _ => first.to_string(),
            };
            let node_id = make_id(&["terraform", scope, &address]);
            let mut node = child_node(file, node_id.clone(), &address, kind, line_number);
            node.metadata.insert("address".to_string(), json!(address));
            nodes.push(node);
            edges.push(contains_edge(
                file,
                &node_id,
                line_number,
                "terraform_block",
            ));
            address_to_id.insert(address, node_id.clone());
            current_owner = Some(node_id);
            brace_depth = count_braces(line);
            in_locals = false;
            continue;
        }

        if line.trim_start().starts_with("locals") && line.contains('{') {
            in_locals = true;
            brace_depth = count_braces(line);
            current_owner = None;
            continue;
        }

        if in_locals {
            if let Some(caps) = local_regex.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
                let address = format!("local.{name}");
                let node_id = make_id(&["terraform", scope, &address]);
                nodes.push(child_node(
                    file,
                    node_id.clone(),
                    &address,
                    "local",
                    line_number,
                ));
                edges.push(contains_edge(
                    file,
                    &node_id,
                    line_number,
                    "terraform_block",
                ));
                address_to_id.insert(address, node_id);
            }
            brace_depth += count_braces(line);
            if brace_depth <= 0 {
                in_locals = false;
            }
            continue;
        }

        if let Some(owner) = &current_owner {
            let relation = if line.contains("depends_on") {
                "depends_on"
            } else {
                "references"
            };
            for address in terraform_references(line) {
                if let Some(target_id) = address_to_id.get(&address) {
                    if target_id != owner {
                        edges.push(Edge::new(
                            owner.clone(),
                            target_id.clone(),
                            relation,
                            CONFIDENCE_EXTRACTED,
                            file.rel_path.clone(),
                            line_location(line_number),
                            Some("terraform_reference".to_string()),
                        ));
                    }
                }
            }
            brace_depth += count_braces(line);
            if brace_depth <= 0 {
                current_owner = None;
            }
        }
    }

    (nodes, edges, ExtractionSidecars::default())
}

fn extract_package_manifest(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let name = Path::new(&file.rel_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let text = String::from_utf8_lossy(bytes);
    let parsed = match name.as_str() {
        "package.json" => parse_package_json(&text, "npm"),
        "composer.json" => parse_package_json(&text, "composer"),
        "cargo.toml" => parse_cargo_toml(&text),
        "pyproject.toml" => parse_pyproject_toml(&text),
        "go.mod" => parse_go_mod(&text),
        "pom.xml" => parse_pom_xml(&text),
        "apm.yml" | "apm.yaml" => parse_apm_yaml(&text),
        _ => None,
    };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);
    let Some(manifest) = parsed else {
        return (nodes, edges, ExtractionSidecars::default());
    };

    let package_id = make_id(&["pkg", &manifest.name]);
    let mut package_node = child_node(file, package_id.clone(), &manifest.name, "package", 1);
    package_node
        .metadata
        .insert("ecosystem".to_string(), json!(manifest.ecosystem));
    if let Some(version) = manifest.version {
        package_node
            .metadata
            .insert("version".to_string(), json!(version));
    }
    nodes.push(package_node);
    edges.push(contains_edge(file, &package_id, 1, "package_manifest"));

    let mut seen = BTreeSet::new();
    for dep in manifest.dependencies {
        if dep.is_empty() || !seen.insert(dep.clone()) {
            continue;
        }
        let dep_id = make_id(&["pkg_ref", &dep]);
        let mut dep_node = child_node(file, dep_id.clone(), &dep, "package_dependency", 1);
        dep_node
            .metadata
            .insert("external".to_string(), json!(true));
        nodes.push(dep_node);
        edges.push(Edge::new(
            package_id.clone(),
            dep_id,
            "depends_on",
            CONFIDENCE_EXTRACTED,
            file.rel_path.clone(),
            line_location(1),
            Some("dependency".to_string()),
        ));
    }

    (nodes, edges, ExtractionSidecars::default())
}

fn extract_mcp_config(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let parsed = serde_json::from_str::<Value>(&text)
        .or_else(|_| serde_json::from_str::<Value>(&strip_jsonc(&text)));
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);
    let Ok(value) = parsed else {
        return (nodes, edges, ExtractionSidecars::default());
    };

    let servers = value
        .get("mcpServers")
        .and_then(Value::as_object)
        .or_else(|| value.pointer("/mcp/servers").and_then(Value::as_object));
    let Some(servers) = servers else {
        if let Some(node) = nodes.first_mut() {
            node.metadata
                .insert("skipped_structure".to_string(), json!("no_mcp_servers_map"));
        }
        return (nodes, edges, ExtractionSidecars::default());
    };

    let file_node_id = file_id(&file.rel_path);
    for (server_name, spec) in servers.iter().take(200) {
        let Some(spec) = spec.as_object() else {
            continue;
        };
        let server_id = make_id(&[&file.rel_path, "mcp_server", server_name]);
        let mut server_node = child_node(file, server_id.clone(), server_name, "mcp_server", 1);
        server_node
            .metadata
            .insert("mcp_kind".to_string(), json!("server"));
        nodes.push(server_node);
        edges.push(Edge::new(
            file_node_id.clone(),
            server_id.clone(),
            "contains",
            CONFIDENCE_EXTRACTED,
            file.rel_path.clone(),
            line_location(1),
            Some("mcp_server".to_string()),
        ));

        if let Some(command) = spec.get("command").and_then(Value::as_str) {
            let command = command.trim();
            if !command.is_empty() {
                let command_id = make_id(&["mcp_command", command]);
                let mut node = child_node(file, command_id.clone(), command, "mcp_command", 1);
                node.metadata
                    .insert("mcp_kind".to_string(), json!("command"));
                nodes.push(node);
                edges.push(Edge::new(
                    server_id.clone(),
                    command_id,
                    "references",
                    CONFIDENCE_EXTRACTED,
                    file.rel_path.clone(),
                    line_location(1),
                    Some("command".to_string()),
                ));
            }
        }

        if let Some(args) = spec.get("args").and_then(Value::as_array) {
            if let Some(package) = detect_mcp_package(args) {
                let package_id = make_id(&["mcp_package", &package]);
                let mut node = child_node(file, package_id.clone(), &package, "mcp_package", 1);
                node.metadata
                    .insert("mcp_kind".to_string(), json!("package"));
                nodes.push(node);
                edges.push(Edge::new(
                    server_id.clone(),
                    package_id,
                    "references",
                    CONFIDENCE_EXTRACTED,
                    file.rel_path.clone(),
                    line_location(1),
                    Some("package".to_string()),
                ));
            }
        }

        if let Some(env) = spec.get("env").and_then(Value::as_object) {
            for env_name in env.keys().take(200) {
                let env_id = make_id(&["env_var", env_name]);
                let mut node = child_node(file, env_id.clone(), env_name, "env_var", 1);
                node.metadata
                    .insert("mcp_kind".to_string(), json!("env_var"));
                nodes.push(node);
                edges.push(Edge::new(
                    server_id.clone(),
                    env_id,
                    "requires_env",
                    CONFIDENCE_EXTRACTED,
                    file.rel_path.clone(),
                    line_location(1),
                    Some("env".to_string()),
                ));
            }
        }
    }

    (nodes, edges, ExtractionSidecars::default())
}

fn extract_pdf(file: &DetectedFile, bytes: &[u8]) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);
    let preview = printable_text_preview(bytes, 4000);
    if let Some(node) = nodes.first_mut() {
        node.metadata
            .insert("magic".to_string(), json!(magic_hex(bytes)));
        node.metadata
            .insert("embedded_text_preview".to_string(), json!(preview));
        node.metadata.insert(
            "page_marker_count".to_string(),
            json!(String::from_utf8_lossy(bytes)
                .matches("/Type /Page")
                .count()),
        );
    }
    if !preview.is_empty() {
        let text_id = make_id(&[&file.rel_path, "embedded_text"]);
        let mut text_node = child_node(file, text_id.clone(), "embedded text", "document_text", 1);
        text_node
            .metadata
            .insert("preview".to_string(), json!(preview));
        nodes.push(text_node);
        edges.push(contains_edge(file, &text_id, 1, "embedded_text"));
    }
    (nodes, edges, ExtractionSidecars::default())
}

fn extract_office(file: &DetectedFile, bytes: &[u8]) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let mut nodes = Vec::new();
    let edges = Vec::new();
    add_file_node(file, &mut nodes);
    let mut preview_parts = Vec::new();
    let mut archive = match zip::ZipArchive::new(Cursor::new(bytes)) {
        Ok(archive) => archive,
        Err(_) => return (nodes, edges, ExtractionSidecars::default()),
    };

    for idx in 0..archive.len().min(300) {
        let Ok(mut entry) = archive.by_index(idx) else {
            continue;
        };
        let name = entry.name().replace('\\', "/");
        if !(name.ends_with(".xml")
            && (name.starts_with("word/") || name.starts_with("xl/") || name.starts_with("ppt/")))
        {
            continue;
        }
        let mut xml = String::new();
        if entry.read_to_string(&mut xml).is_ok() {
            let text = xml_text_preview(&xml, 1000);
            if !text.is_empty() {
                preview_parts.push(text);
            }
        }
    }
    let preview = preview_parts
        .join("\n")
        .chars()
        .take(4000)
        .collect::<String>();
    if let Some(node) = nodes.first_mut() {
        node.metadata
            .insert("text_preview".to_string(), json!(preview));
        node.metadata
            .insert("container".to_string(), json!("zip_xml"));
    }
    (nodes, edges, ExtractionSidecars::default())
}

fn extract_image(file: &DetectedFile, bytes: &[u8]) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let mut nodes = Vec::new();
    add_file_node(file, &mut nodes);
    if let Some(node) = nodes.first_mut() {
        node.metadata
            .insert("magic".to_string(), json!(magic_hex(bytes)));
        if let Some((width, height)) = image_dimensions(file, bytes) {
            node.metadata.insert("width".to_string(), json!(width));
            node.metadata.insert("height".to_string(), json!(height));
        }
        if file.rel_path.to_ascii_lowercase().ends_with(".svg") {
            let text = String::from_utf8_lossy(bytes);
            if let Some(view_box) = Regex::new(r#"viewBox\s*=\s*"([^"]+)""#)
                .expect("valid svg regex")
                .captures(&text)
                .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
            {
                node.metadata
                    .insert("view_box".to_string(), json!(view_box));
            }
        }
    }
    (nodes, Vec::new(), ExtractionSidecars::default())
}

fn extract_audio_video(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let mut nodes = Vec::new();
    add_file_node(file, &mut nodes);
    if let Some(node) = nodes.first_mut() {
        node.metadata
            .insert("magic".to_string(), json!(magic_hex(bytes)));
        node.metadata
            .insert("metadata_mode".to_string(), json!("container_header_only"));
    }
    (nodes, Vec::new(), ExtractionSidecars::default())
}

fn extract_archive(
    file: &DetectedFile,
    bytes: &[u8],
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    add_file_node(file, &mut nodes);
    let lower = file.rel_path.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        add_zip_entries(file, bytes, &mut nodes, &mut edges);
    } else if lower.ends_with(".tar") {
        add_tar_entries(file, Cursor::new(bytes), &mut nodes, &mut edges);
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
        add_tar_entries(file, decoder, &mut nodes, &mut edges);
    } else if lower.ends_with(".gz") {
        if let Some(node) = nodes.first_mut() {
            node.metadata
                .insert("archive_kind".to_string(), json!("gzip_stream"));
        }
    }
    (nodes, edges, ExtractionSidecars::default())
}

#[derive(Debug)]
struct PackageManifest {
    ecosystem: String,
    name: String,
    version: Option<String>,
    dependencies: Vec<String>,
}

fn child_node(
    file: &DetectedFile,
    id: impl Into<String>,
    label: impl Into<String>,
    node_type: impl Into<String>,
    line: usize,
) -> Node {
    let mut node = Node::new(
        id,
        label,
        node_type,
        Some(file.language_name.clone()),
        file.rel_path.clone(),
        line_location(line),
    );
    node.file_type = file.file_category.graph_file_type().to_string();
    node
}

fn contains_edge(file: &DetectedFile, target: &str, line: usize, context: &str) -> Edge {
    Edge::new(
        file_id(&file.rel_path),
        target.to_string(),
        "contains",
        CONFIDENCE_EXTRACTED,
        file.rel_path.clone(),
        line_location(line),
        Some(context.to_string()),
    )
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

fn clean_html_text(text: &str) -> String {
    let no_tags = Regex::new(r"(?is)<[^>]+>")
        .expect("valid html regex")
        .replace_all(text, " ");
    decode_basic_entities(&no_tags)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_basic_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn line_number_for_offset(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                }
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            continue;
        }
        out.push(ch);
    }
    Regex::new(r",\s*([}\]])")
        .expect("valid jsonc regex")
        .replace_all(&out, "$1")
        .to_string()
}

fn looks_like_config_json(file: &DetectedFile, value: &Value) -> bool {
    let name = Path::new(&file.rel_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if [
        "tsconfig.json",
        "jsconfig.json",
        "deno.json",
        "deno.jsonc",
        "biome.json",
        "eslint.config.json",
        "package-lock.json",
    ]
    .iter()
    .any(|known| name == *known)
    {
        return true;
    }
    let Some(map) = value.as_object() else {
        return false;
    };
    [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "compilerOptions",
        "scripts",
        "extends",
        "$schema",
        "$ref",
        "mcpServers",
    ]
    .iter()
    .any(|key| map.contains_key(*key))
}

fn add_object_keys(
    file: &DetectedFile,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    parent_id: &str,
    parent_key: Option<&str>,
    map: &serde_json::Map<String, Value>,
    depth: usize,
) {
    if depth > 5 {
        return;
    }
    for (idx, (key, value)) in map.iter().take(500).enumerate() {
        let node_id = make_id(&[&file.rel_path, "json", parent_key.unwrap_or("root"), key]);
        nodes.push(child_node(
            file,
            node_id.clone(),
            key,
            "config_key",
            idx + 1,
        ));
        edges.push(Edge::new(
            parent_id.to_string(),
            node_id.clone(),
            "contains",
            CONFIDENCE_EXTRACTED,
            file.rel_path.clone(),
            line_location(idx + 1),
            Some("config".to_string()),
        ));
        if let Value::Object(child) = value {
            add_object_keys(file, nodes, edges, &node_id, Some(key), child, depth + 1);
        }
    }
}

fn is_kubernetes_doc(value: &serde_yaml::Value) -> bool {
    yaml_get(value, "apiVersion").is_some()
        && yaml_get(value, "kind")
            .and_then(serde_yaml::Value::as_str)
            .is_some()
        && yaml_get(value, "metadata")
            .and_then(|metadata| yaml_get(metadata, "name"))
            .and_then(serde_yaml::Value::as_str)
            .is_some()
}

fn extract_kubernetes_docs(
    file: &DetectedFile,
    docs: &[serde_yaml::Value],
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
) {
    let mut resource_ids = BTreeMap::new();
    for value in docs {
        let Some((kind, name, namespace)) = k8s_identity(value) else {
            continue;
        };
        let resource_id = make_id(&["k8s", &namespace, &kind, &name]);
        let mut node = child_node(
            file,
            resource_id.clone(),
            format!("{kind}/{name}"),
            "kubernetes_resource",
            1,
        );
        node.metadata.insert("kind".to_string(), json!(kind));
        node.metadata.insert("name".to_string(), json!(name));
        node.metadata
            .insert("namespace".to_string(), json!(namespace.clone()));
        nodes.push(node);
        edges.push(contains_edge(file, &resource_id, 1, "kubernetes_resource"));
        resource_ids.insert((namespace, kind, name), resource_id);
    }

    for value in docs {
        let Some((kind, name, namespace)) = k8s_identity(value) else {
            continue;
        };
        let Some(source_id) = resource_ids.get(&(namespace.clone(), kind, name)) else {
            continue;
        };
        for (target_kind, target_name) in k8s_named_refs(value) {
            if let Some(target_id) =
                resource_ids.get(&(namespace.clone(), target_kind.clone(), target_name.clone()))
            {
                edges.push(Edge::new(
                    source_id.clone(),
                    target_id.clone(),
                    "references",
                    CONFIDENCE_EXTRACTED,
                    file.rel_path.clone(),
                    line_location(1),
                    Some("kubernetes_reference".to_string()),
                ));
            }
        }
    }
}

fn k8s_identity(value: &serde_yaml::Value) -> Option<(String, String, String)> {
    let kind = yaml_get(value, "kind")?.as_str()?.to_string();
    let metadata = yaml_get(value, "metadata")?;
    let name = yaml_get(metadata, "name")?.as_str()?.to_string();
    let namespace = yaml_get(metadata, "namespace")
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("default")
        .to_string();
    Some((kind, name, namespace))
}

fn k8s_named_refs(value: &serde_yaml::Value) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    collect_k8s_refs(value, &mut refs);
    refs.sort();
    refs.dedup();
    refs
}

fn collect_k8s_refs(value: &serde_yaml::Value, refs: &mut Vec<(String, String)>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (key, value) in map {
                if let Some(key) = yaml_key_string(key) {
                    let kind = match key.as_str() {
                        "configMapRef" | "configMap" => Some("ConfigMap"),
                        "secretRef" | "secret" => Some("Secret"),
                        "serviceAccountName" => Some("ServiceAccount"),
                        "persistentVolumeClaim" => Some("PersistentVolumeClaim"),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        if let Some(name) = yaml_get(value, "name")
                            .and_then(serde_yaml::Value::as_str)
                            .or_else(|| value.as_str())
                        {
                            refs.push((kind.to_string(), name.to_string()));
                        }
                    }
                }
                collect_k8s_refs(value, refs);
            }
        }
        serde_yaml::Value::Sequence(items) => {
            for item in items {
                collect_k8s_refs(item, refs);
            }
        }
        _ => {}
    }
}

fn yaml_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    let serde_yaml::Value::Mapping(map) = value else {
        return None;
    };
    map.get(serde_yaml::Value::String(key.to_string()))
}

fn yaml_key_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(value) => Some(value.clone()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn terraform_references(line: &str) -> Vec<String> {
    let regex = Regex::new(
        r#"\b(var|local|module)\.([A-Za-z0-9_-]+)|\bdata\.([A-Za-z0-9_-]+)\.([A-Za-z0-9_-]+)|\b([A-Za-z0-9]+_[A-Za-z0-9_]+)\.([A-Za-z0-9_-]+)"#,
    )
    .expect("valid terraform regex");
    regex
        .captures_iter(line)
        .filter_map(|caps| {
            if let (Some(head), Some(name)) = (caps.get(1), caps.get(2)) {
                return Some(format!("{}.{}", head.as_str(), name.as_str()));
            }
            if let (Some(kind), Some(name)) = (caps.get(3), caps.get(4)) {
                return Some(format!("data.{}.{}", kind.as_str(), name.as_str()));
            }
            if let (Some(kind), Some(name)) = (caps.get(5), caps.get(6)) {
                return Some(format!("{}.{}", kind.as_str(), name.as_str()));
            }
            None
        })
        .collect()
}

fn count_braces(line: &str) -> i32 {
    line.chars().filter(|ch| *ch == '{').count() as i32
        - line.chars().filter(|ch| *ch == '}').count() as i32
}

fn parse_package_json(text: &str, ecosystem: &str) -> Option<PackageManifest> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    let map = value.as_object()?;
    let name = map.get("name")?.as_str()?.to_string();
    let version = map
        .get("version")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut dependencies = Vec::new();
    for key in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
        "require",
        "require-dev",
    ] {
        if let Some(deps) = map.get(key).and_then(Value::as_object) {
            dependencies.extend(deps.keys().cloned());
        }
    }
    Some(PackageManifest {
        ecosystem: ecosystem.to_string(),
        name,
        version,
        dependencies,
    })
}

fn parse_cargo_toml(text: &str) -> Option<PackageManifest> {
    let value = toml::from_str::<toml::Value>(text).ok()?;
    let package = value.get("package")?.as_table()?;
    let name = package.get("name")?.as_str()?.to_string();
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .map(ToString::to_string);
    let mut dependencies = Vec::new();
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = value.get(key).and_then(toml::Value::as_table) {
            dependencies.extend(table.keys().cloned());
        }
    }
    Some(PackageManifest {
        ecosystem: "rust".to_string(),
        name,
        version,
        dependencies,
    })
}

fn parse_pyproject_toml(text: &str) -> Option<PackageManifest> {
    let value = toml::from_str::<toml::Value>(text).ok()?;
    let project = value.get("project").and_then(toml::Value::as_table);
    let poetry = value
        .get("tool")
        .and_then(toml::Value::as_table)
        .and_then(|tool| tool.get("poetry"))
        .and_then(toml::Value::as_table);
    let name = project
        .and_then(|table| table.get("name"))
        .and_then(toml::Value::as_str)
        .or_else(|| {
            poetry
                .and_then(|table| table.get("name"))
                .and_then(toml::Value::as_str)
        })?
        .to_string();
    let version = project
        .and_then(|table| table.get("version"))
        .and_then(toml::Value::as_str)
        .or_else(|| {
            poetry
                .and_then(|table| table.get("version"))
                .and_then(toml::Value::as_str)
        })
        .map(ToString::to_string);
    let mut dependencies = Vec::new();
    if let Some(items) = project
        .and_then(|table| table.get("dependencies"))
        .and_then(toml::Value::as_array)
    {
        dependencies.extend(
            items
                .iter()
                .filter_map(toml::Value::as_str)
                .map(pep508_name),
        );
    }
    if let Some(table) = poetry
        .and_then(|table| table.get("dependencies"))
        .and_then(toml::Value::as_table)
    {
        dependencies.extend(table.keys().filter(|key| key.as_str() != "python").cloned());
    }
    Some(PackageManifest {
        ecosystem: "python".to_string(),
        name,
        version,
        dependencies,
    })
}

fn parse_go_mod(text: &str) -> Option<PackageManifest> {
    let module_regex = Regex::new(r"(?m)^\s*module\s+(\S+)").expect("valid go.mod regex");
    let require_regex = Regex::new(r"(?m)^\s*(?:require\s+)?([A-Za-z0-9_.\-/]+)\s+v[0-9]")
        .expect("valid go.mod regex");
    let name = module_regex
        .captures(text)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))?;
    let dependencies = require_regex
        .captures_iter(text)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .filter(|dep| dep != &name)
        .collect();
    Some(PackageManifest {
        ecosystem: "go".to_string(),
        name,
        version: None,
        dependencies,
    })
}

fn parse_pom_xml(text: &str) -> Option<PackageManifest> {
    let artifact = xml_tag_text(text, "artifactId")?;
    let group = xml_tag_text(text, "groupId");
    let name = group
        .as_ref()
        .map(|group| format!("{group}:{artifact}"))
        .unwrap_or(artifact);
    let version = xml_tag_text(text, "version");
    let dependencies = Regex::new(r"(?is)<dependency\b.*?</dependency>")
        .expect("valid pom regex")
        .find_iter(text)
        .filter_map(|m| {
            let block = m.as_str();
            let artifact = xml_tag_text(block, "artifactId")?;
            let group = xml_tag_text(block, "groupId");
            Some(
                group
                    .as_ref()
                    .map(|group| format!("{group}:{artifact}"))
                    .unwrap_or(artifact),
            )
        })
        .collect();
    Some(PackageManifest {
        ecosystem: "maven".to_string(),
        name,
        version,
        dependencies,
    })
}

fn parse_apm_yaml(text: &str) -> Option<PackageManifest> {
    let value = serde_yaml::from_str::<serde_yaml::Value>(text).ok()?;
    let name = yaml_get(&value, "name")?.as_str()?.to_string();
    let version = yaml_get(&value, "version")
        .and_then(serde_yaml::Value::as_str)
        .map(ToString::to_string);
    let dependencies = yaml_get(&value, "dependencies")
        .and_then(|deps| match deps {
            serde_yaml::Value::Mapping(map) => {
                Some(map.keys().filter_map(yaml_key_string).collect())
            }
            serde_yaml::Value::Sequence(items) => Some(
                items
                    .iter()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(ToString::to_string)
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    Some(PackageManifest {
        ecosystem: "apm".to_string(),
        name,
        version,
        dependencies,
    })
}

fn pep508_name(spec: &str) -> String {
    spec.split(|ch: char| ch.is_whitespace() || "<>=!~;[(".contains(ch))
        .next()
        .unwrap_or(spec)
        .to_string()
}

fn xml_tag_text(text: &str, tag: &str) -> Option<String> {
    let regex = Regex::new(&format!(r"(?is)<{tag}[^>]*>(.*?)</{tag}>")).ok()?;
    regex
        .captures(text)
        .and_then(|caps| {
            caps.get(1)
                .map(|m| decode_basic_entities(m.as_str()).trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

fn detect_mcp_package(args: &[Value]) -> Option<String> {
    let npm_regex = Regex::new(r"^@[a-z0-9][a-z0-9._-]*/[a-z0-9][a-z0-9._-]*(?:@[\w.\-+]+)?$")
        .expect("valid package regex");
    let py_regex =
        Regex::new(r"^[a-z0-9][a-z0-9._-]*-mcp(?:-[a-z0-9._-]+)?$|^mcp-[a-z0-9][a-z0-9._-]*$")
            .expect("valid package regex");
    for arg in args.iter().filter_map(Value::as_str) {
        let arg = arg.trim();
        if arg.is_empty() || arg.starts_with('-') {
            continue;
        }
        if npm_regex.is_match(arg) {
            return Some(strip_npm_version(arg));
        }
        if py_regex.is_match(arg) {
            return Some(arg.to_string());
        }
    }
    None
}

fn strip_npm_version(package: &str) -> String {
    if package.starts_with('@') {
        if let Some(index) = package[1..].find('@') {
            return package[..index + 1].to_string();
        }
        return package.to_string();
    }
    package.split('@').next().unwrap_or(package).to_string()
}

fn printable_text_preview(bytes: &[u8], max_chars: usize) -> String {
    let mut runs = Vec::new();
    let mut current = String::new();
    for byte in bytes {
        let ch = *byte as char;
        if ch.is_ascii_graphic() || ch.is_ascii_whitespace() {
            current.push(ch);
        } else {
            if current.trim().len() >= 20 {
                runs.push(current.trim().to_string());
            }
            current.clear();
        }
        if runs.iter().map(String::len).sum::<usize>() >= max_chars {
            break;
        }
    }
    if current.trim().len() >= 20 {
        runs.push(current.trim().to_string());
    }
    runs.join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn xml_text_preview(xml: &str, max_chars: usize) -> String {
    let no_tags = Regex::new(r"(?is)<[^>]+>")
        .expect("valid xml regex")
        .replace_all(xml, " ");
    decode_basic_entities(&no_tags)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn magic_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

fn image_dimensions(file: &DetectedFile, bytes: &[u8]) -> Option<(u32, u32)> {
    let lower = file.rel_path.to_ascii_lowercase();
    if lower.ends_with(".png") && bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((width, height));
    }
    if lower.ends_with(".gif")
        && bytes.len() >= 10
        && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
    {
        let width = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
        let height = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
        return Some((width, height));
    }
    if (lower.ends_with(".jpg") || lower.ends_with(".jpeg")) && bytes.starts_with(&[0xff, 0xd8]) {
        return jpeg_dimensions(bytes);
    }
    if lower.ends_with(".svg") {
        let text = String::from_utf8_lossy(bytes);
        let width = svg_dimension(&text, "width");
        let height = svg_dimension(&text, "height");
        if let (Some(width), Some(height)) = (width, height) {
            return Some((width, height));
        }
    }
    None
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut index = 2;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        let length = u16::from_be_bytes(bytes[index + 2..index + 4].try_into().ok()?) as usize;
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            let height = u16::from_be_bytes(bytes[index + 5..index + 7].try_into().ok()?) as u32;
            let width = u16::from_be_bytes(bytes[index + 7..index + 9].try_into().ok()?) as u32;
            return Some((width, height));
        }
        if length < 2 {
            return None;
        }
        index += 2 + length;
    }
    None
}

fn svg_dimension(text: &str, name: &str) -> Option<u32> {
    let regex = Regex::new(&format!(r#"{name}\s*=\s*"([0-9.]+)"#)).ok()?;
    let value = regex.captures(text)?.get(1)?.as_str().parse::<f32>().ok()?;
    Some(value.round() as u32)
}

fn add_zip_entries(
    file: &DetectedFile,
    bytes: &[u8],
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
) {
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return;
    };
    for idx in 0..archive.len().min(200) {
        let Ok(entry) = archive.by_index(idx) else {
            continue;
        };
        let name = entry.name().replace('\\', "/");
        let node_id = make_id(&[&file.rel_path, "archive_entry", &name]);
        let mut node = child_node(file, node_id.clone(), &name, "archive_entry", 1);
        node.metadata
            .insert("size".to_string(), json!(entry.size()));
        node.metadata.insert(
            "compressed_size".to_string(),
            json!(entry.compressed_size()),
        );
        nodes.push(node);
        edges.push(contains_edge(file, &node_id, 1, "archive_entry"));
    }
}

fn add_tar_entries<R: Read>(
    file: &DetectedFile,
    reader: R,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
) {
    let mut archive = tar::Archive::new(reader);
    let Ok(entries) = archive.entries() else {
        return;
    };
    for entry in entries.take(200).flatten() {
        let path = entry
            .path()
            .ok()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| "entry".to_string());
        let node_id = make_id(&[&file.rel_path, "archive_entry", &path]);
        let mut node = child_node(file, node_id.clone(), &path, "archive_entry", 1);
        if let Ok(size) = entry.header().size() {
            node.metadata.insert("size".to_string(), json!(size));
        }
        nodes.push(node);
        edges.push(contains_edge(file, &node_id, 1, "archive_entry"));
    }
}

fn resolve_document_link(
    source_file: &str,
    raw: &str,
    file_lookup: &BTreeMap<String, String>,
) -> Option<String> {
    let link = raw
        .split('#')
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('<')
        .trim_matches('>');
    if link.is_empty()
        || link.starts_with("http://")
        || link.starts_with("https://")
        || link.starts_with("mailto:")
        || link.starts_with('#')
    {
        return None;
    }
    let source_dir = Path::new(source_file)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let joined = normalize_path(source_dir.join(link));
    for candidate in path_candidates(&joined) {
        if let Some(id) = file_lookup.get(&candidate) {
            return Some(id.clone());
        }
    }
    file_lookup.get(&joined).cloned()
}

fn tree_sitter_language(language: SupportedLanguage) -> Language {
    match language {
        SupportedLanguage::Python => tree_sitter_python::LANGUAGE.into(),
        SupportedLanguage::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        SupportedLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        SupportedLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        SupportedLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
        SupportedLanguage::Go => tree_sitter_go::LANGUAGE.into(),
        SupportedLanguage::Java => tree_sitter_java::LANGUAGE.into(),
        SupportedLanguage::C => tree_sitter_c::LANGUAGE.into(),
        SupportedLanguage::Cpp => tree_sitter_cpp::LANGUAGE.into(),
    }
}

fn walk_tree(
    node: TsNode<'_>,
    source: &[u8],
    language: SupportedLanguage,
    file_lookup: &BTreeMap<String, String>,
    state: WalkState<'_>,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    sidecars: &mut ExtractionSidecars,
) {
    let kind = node.kind();
    if is_import_node(kind, language) {
        add_imports_from_node(node, source, language, &state, nodes, edges, sidecars);
    }

    let mut next_state = WalkState {
        file: state.file,
        file_id: state.file_id.clone(),
        current_callable: state.current_callable.clone(),
        class_stack: state.class_stack.clone(),
    };

    if let Some(definition_kind) = definition_kind(kind, language, node, source) {
        if let Some(name) = extract_definition_name(node, source, language) {
            let line = node.start_position().row + 1;
            let node_id = symbol_id(
                &state.file.rel_path,
                &scoped_name(&state.class_stack, &name),
            );
            let mut graph_node = Node::new(
                node_id.clone(),
                name.clone(),
                definition_kind,
                Some(state.file.language_name.clone()),
                state.file.rel_path.clone(),
                line_location(line),
            );
            if !state.class_stack.is_empty() {
                graph_node
                    .metadata
                    .insert("scope_chain".to_string(), json!(state.class_stack.clone()));
            }
            nodes.push(graph_node);
            sidecars.definitions.push(DefinitionRecord {
                id: node_id.clone(),
                label: name.clone(),
                file_id: state.file_id.clone(),
                source_file: state.file.rel_path.clone(),
            });
            edges.push(Edge::new(
                state.file_id.clone(),
                node_id.clone(),
                "contains",
                CONFIDENCE_EXTRACTED,
                state.file.rel_path.clone(),
                line_location(line),
                Some("definition".to_string()),
            ));
            if let Some(parent_class) = state.class_stack.last() {
                edges.push(Edge::new(
                    parent_class.clone(),
                    node_id.clone(),
                    if definition_kind == "method" {
                        "method"
                    } else {
                        "contains"
                    },
                    CONFIDENCE_EXTRACTED,
                    state.file.rel_path.clone(),
                    line_location(line),
                    Some("definition".to_string()),
                ));
            }
            if definition_kind == "class"
                || definition_kind == "struct"
                || definition_kind == "interface"
                || definition_kind == "trait"
            {
                next_state.class_stack.push(node_id.clone());
            }
            if matches!(definition_kind, "function" | "method" | "constructor") {
                next_state.current_callable = Some(node_id);
            }
        }
    }

    if is_call_node(kind, language) {
        if let Some(caller_id) = &state.current_callable {
            if let Some(callee) = extract_call_name(node, source, language) {
                sidecars.raw_calls.push(RawCall {
                    caller_id: caller_id.clone(),
                    callee,
                    source_file: state.file.rel_path.clone(),
                    source_location: line_location(node.start_position().row + 1),
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            walk_tree(
                child,
                source,
                language,
                file_lookup,
                WalkState {
                    file: next_state.file,
                    file_id: next_state.file_id.clone(),
                    current_callable: next_state.current_callable.clone(),
                    class_stack: next_state.class_stack.clone(),
                },
                nodes,
                edges,
                sidecars,
            );
        }
    }
}

fn extract_heuristic(
    file: &DetectedFile,
    bytes: &[u8],
    _file_lookup: &BTreeMap<String, String>,
) -> (Vec<Node>, Vec<Edge>, ExtractionSidecars) {
    let text = String::from_utf8_lossy(bytes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut sidecars = ExtractionSidecars::default();
    let file_node_id = file_id(&file.rel_path);
    add_file_node(file, &mut nodes);

    let definition_regex = Regex::new(
        r"(?x)
        ^\s*
        (?:
            class\s+([A-Za-z_][A-Za-z0-9_]*) |
            def\s+([A-Za-z_][A-Za-z0-9_]*) |
            function\s+([A-Za-z_][A-Za-z0-9_]*) |
            fn\s+([A-Za-z_][A-Za-z0-9_]*) |
            (?:pub\s+)?(?:async\s+)?(?:func|proc)\s+([A-Za-z_][A-Za-z0-9_]*)
        )
        ",
    )
    .expect("valid definition regex");
    let import_regex = Regex::new(
        r#"(?x)
        ^\s*
        (?:
            import\s+([^;"']+) |
            from\s+([A-Za-z0-9_./:-]+)\s+import |
            use\s+([^;]+) |
            require\s*\(\s*["']([^"']+)["']\s*\) |
            source\s+["']?([^"'\s]+)
        )
        "#,
    )
    .expect("valid import regex");

    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if let Some(caps) = definition_regex.captures(line) {
            let name = (1..=5)
                .filter_map(|idx| caps.get(idx).map(|m| m.as_str()))
                .next()
                .unwrap_or_default();
            if !name.is_empty() {
                let node_id = symbol_id(&file.rel_path, name);
                nodes.push(Node::new(
                    node_id.clone(),
                    name,
                    if line.trim_start().starts_with("class ") {
                        "class"
                    } else {
                        "function"
                    },
                    Some(file.language_name.clone()),
                    file.rel_path.clone(),
                    line_location(line_number),
                ));
                sidecars.definitions.push(DefinitionRecord {
                    id: node_id.clone(),
                    label: name.to_string(),
                    file_id: file_node_id.clone(),
                    source_file: file.rel_path.clone(),
                });
                edges.push(Edge::new(
                    file_node_id.clone(),
                    node_id,
                    "contains",
                    CONFIDENCE_EXTRACTED,
                    file.rel_path.clone(),
                    line_location(line_number),
                    Some("definition".to_string()),
                ));
            }
        }

        if let Some(caps) = import_regex.captures(line) {
            for idx in 1..=5 {
                if let Some(specifier) = caps.get(idx).map(|m| m.as_str().trim()) {
                    if !specifier.is_empty() {
                        add_import_record(
                            file,
                            &file_node_id,
                            specifier,
                            line_number,
                            &mut nodes,
                            &mut edges,
                            &mut sidecars,
                        );
                    }
                }
            }
        }
    }

    (nodes, edges, sidecars)
}

fn add_file_node(file: &DetectedFile, nodes: &mut Vec<Node>) {
    let mut node = Node::new(
        file_id(&file.rel_path),
        Path::new(&file.rel_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&file.rel_path),
        "file",
        Some(file.language_name.clone()),
        file.rel_path.clone(),
        line_location(1),
    );
    node.file_type = file.file_category.graph_file_type().to_string();
    node.metadata
        .insert("path".to_string(), json!(file.rel_path.clone()));
    node.metadata.insert(
        "byte_size".to_string(),
        json!(file
            .path
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0)),
    );
    nodes.push(node);
}

fn add_imports_from_node(
    node: TsNode<'_>,
    source: &[u8],
    language: SupportedLanguage,
    state: &WalkState<'_>,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    sidecars: &mut ExtractionSidecars,
) {
    let text = node_text(node, source);
    if matches!(
        language,
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript | SupportedLanguage::Tsx
    ) && node.kind() == "call_expression"
        && !(text.starts_with("import(") || text.contains("require("))
    {
        return;
    }
    for specifier in import_specifiers(&text, language) {
        add_import_record(
            state.file,
            &state.file_id,
            &specifier,
            node.start_position().row + 1,
            nodes,
            edges,
            sidecars,
        );
    }
}

fn add_import_record(
    file: &DetectedFile,
    file_node_id: &str,
    specifier: &str,
    line_number: usize,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    sidecars: &mut ExtractionSidecars,
) {
    let cleaned = clean_import_specifier(specifier);
    if cleaned.is_empty() {
        return;
    }
    let import_node_id = import_id(&file.rel_path, &cleaned);
    let mut node = Node::new(
        import_node_id.clone(),
        cleaned.clone(),
        "import",
        Some(file.language_name.clone()),
        file.rel_path.clone(),
        line_location(line_number),
    );
    node.metadata
        .insert("specifier".to_string(), json!(cleaned));
    nodes.push(node);
    edges.push(Edge::new(
        file_node_id.to_string(),
        import_node_id,
        "imports",
        CONFIDENCE_EXTRACTED,
        file.rel_path.clone(),
        line_location(line_number),
        Some("import".to_string()),
    ));
    sidecars.imports.push(ImportRecord {
        file_id: file_node_id.to_string(),
        specifier: clean_import_specifier(specifier),
        source_file: file.rel_path.clone(),
        source_location: line_location(line_number),
    });
}

fn resolve_imports(
    builder: &mut GraphBuilder,
    imports: &[ImportRecord],
    file_lookup: &BTreeMap<String, String>,
) {
    for import in imports {
        if let Some(target_id) =
            resolve_local_import(&import.source_file, &import.specifier, file_lookup)
        {
            if builder.contains_node(&target_id) {
                builder.add_edge(Edge::new(
                    import.file_id.clone(),
                    target_id,
                    "imports_from",
                    CONFIDENCE_EXTRACTED,
                    import.source_file.clone(),
                    import.source_location.clone(),
                    Some("import".to_string()),
                ));
            }
        }
    }
}

fn resolve_calls(builder: &mut GraphBuilder, sidecars: &ExtractionSidecars) {
    let mut label_index: BTreeMap<String, Vec<&DefinitionRecord>> = BTreeMap::new();
    let mut node_file: BTreeMap<String, String> = BTreeMap::new();
    for definition in &sidecars.definitions {
        label_index
            .entry(definition.label.to_ascii_lowercase())
            .or_default()
            .push(definition);
        node_file.insert(definition.id.clone(), definition.file_id.clone());
    }

    let mut import_evidence: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for import in &sidecars.imports {
        import_evidence
            .entry(import.file_id.clone())
            .or_default()
            .insert(import.specifier.to_ascii_lowercase());
    }

    for raw_call in &sidecars.raw_calls {
        if is_builtin_call(&raw_call.callee) {
            continue;
        }
        let key = raw_call.callee.to_ascii_lowercase();
        let Some(candidates) = label_index.get(&key) else {
            continue;
        };
        let caller_file = node_file
            .get(&raw_call.caller_id)
            .cloned()
            .unwrap_or_default();
        let target = choose_call_target(candidates, &caller_file, &raw_call.source_file);
        let Some(target) = target else {
            continue;
        };
        if target.id == raw_call.caller_id {
            continue;
        }
        let confidence = if target.file_id == caller_file
            || import_evidence
                .get(&caller_file)
                .map(|imports| imports.iter().any(|item| item.contains(&key)))
                .unwrap_or(false)
        {
            CONFIDENCE_EXTRACTED
        } else {
            CONFIDENCE_INFERRED
        };
        builder.add_edge(Edge::new(
            raw_call.caller_id.clone(),
            target.id.clone(),
            "calls",
            confidence,
            raw_call.source_file.clone(),
            raw_call.source_location.clone(),
            Some("call".to_string()),
        ));
    }
}

fn choose_call_target<'a>(
    candidates: &'a [&DefinitionRecord],
    caller_file_id: &str,
    caller_source_file: &str,
) -> Option<&'a DefinitionRecord> {
    let same_file = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.file_id == caller_file_id)
        .collect::<Vec<_>>();
    if same_file.len() == 1 {
        return same_file.first().copied();
    }
    if candidates.len() == 1 {
        return candidates.first().copied();
    }
    let non_test = candidates
        .iter()
        .copied()
        .filter(|candidate| !looks_like_test_path(&candidate.source_file))
        .collect::<Vec<_>>();
    if non_test.len() == 1 && looks_like_test_path(caller_source_file) {
        return non_test.first().copied();
    }
    None
}

fn build_file_lookup(files: &[DetectedFile]) -> BTreeMap<String, String> {
    let mut lookup = BTreeMap::new();
    for file in files {
        lookup.insert(file.rel_path.clone(), file_id(&file.rel_path));
    }
    lookup
}

fn resolve_local_import(
    source_file: &str,
    specifier: &str,
    file_lookup: &BTreeMap<String, String>,
) -> Option<String> {
    let specifier = clean_import_specifier(specifier);
    let source_dir = Path::new(source_file)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut candidates = Vec::new();

    if specifier.starts_with('.') || specifier.starts_with('/') {
        let joined = normalize_path(source_dir.join(specifier.trim_start_matches('/')));
        candidates.extend(path_candidates(&joined));
    } else if specifier.starts_with("crate::") {
        let module = specifier.trim_start_matches("crate::").replace("::", "/");
        candidates.extend(path_candidates(&format!("src/{module}")));
    } else if specifier.contains("::") {
        candidates.extend(path_candidates(&specifier.replace("::", "/")));
    } else if specifier.contains('.') && !specifier.contains('/') {
        candidates.extend(path_candidates(&specifier.replace('.', "/")));
    } else if specifier.contains('/') {
        candidates.extend(path_candidates(&specifier));
    }

    for candidate in candidates {
        if let Some(file_id) = file_lookup.get(&candidate) {
            return Some(file_id.clone());
        }
    }
    None
}

fn path_candidates(base: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let base = base.trim_start_matches("./").replace('\\', "/");
    candidates.push(base.clone());
    for ext in [
        ".py", ".pyi", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".rs", ".go", ".java", ".c", ".h",
        ".cc", ".cpp", ".cxx", ".hpp", ".hh", ".hxx", ".rb", ".php", ".swift", ".kt", ".cs",
    ] {
        candidates.push(format!("{base}{ext}"));
    }
    for index in [
        "index.ts",
        "index.tsx",
        "index.js",
        "index.jsx",
        "__init__.py",
        "mod.rs",
    ] {
        candidates.push(format!("{base}/{index}"));
    }
    dedupe_preserving_order(candidates)
}

fn normalize_path(path: PathBuf) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
        .trim_start_matches("./")
        .to_string()
}

fn definition_kind(
    kind: &str,
    language: SupportedLanguage,
    node: TsNode<'_>,
    source: &[u8],
) -> Option<&'static str> {
    match language {
        SupportedLanguage::Python => match kind {
            "class_definition" => Some("class"),
            "function_definition" => Some("function"),
            _ => None,
        },
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript | SupportedLanguage::Tsx => {
            match kind {
                "class_declaration" => Some("class"),
                "function_declaration" | "generator_function_declaration" => Some("function"),
                "method_definition" | "method_signature" | "abstract_method_signature" => {
                    Some("method")
                }
                "lexical_declaration" | "variable_declaration" => {
                    if declaration_contains_function(node, source) {
                        Some("function")
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        SupportedLanguage::Rust => match kind {
            "function_item" => Some("function"),
            "struct_item" => Some("struct"),
            "enum_item" => Some("enum"),
            "trait_item" => Some("trait"),
            "mod_item" => Some("module"),
            _ => None,
        },
        SupportedLanguage::Go => match kind {
            "function_declaration" => Some("function"),
            "method_declaration" => Some("method"),
            "type_declaration" => Some("symbol"),
            _ => None,
        },
        SupportedLanguage::Java => match kind {
            "class_declaration" => Some("class"),
            "interface_declaration" => Some("interface"),
            "enum_declaration" => Some("enum"),
            "method_declaration" => Some("method"),
            "constructor_declaration" => Some("constructor"),
            _ => None,
        },
        SupportedLanguage::C | SupportedLanguage::Cpp => match kind {
            "function_definition" => Some("function"),
            "struct_specifier" => Some("struct"),
            "class_specifier" => Some("class"),
            "enum_specifier" => Some("enum"),
            _ => None,
        },
    }
}

fn is_import_node(kind: &str, language: SupportedLanguage) -> bool {
    match language {
        SupportedLanguage::Python => matches!(kind, "import_statement" | "import_from_statement"),
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript | SupportedLanguage::Tsx => {
            matches!(
                kind,
                "import_statement" | "export_statement" | "call_expression"
            )
        }
        SupportedLanguage::Rust => matches!(kind, "use_declaration" | "mod_item"),
        SupportedLanguage::Go => matches!(kind, "import_declaration" | "import_spec"),
        SupportedLanguage::Java => matches!(kind, "import_declaration"),
        SupportedLanguage::C | SupportedLanguage::Cpp => matches!(kind, "preproc_include"),
    }
}

fn is_call_node(kind: &str, language: SupportedLanguage) -> bool {
    match language {
        SupportedLanguage::Java => {
            matches!(kind, "method_invocation" | "object_creation_expression")
        }
        _ => matches!(kind, "call" | "call_expression"),
    }
}

fn extract_definition_name(
    node: TsNode<'_>,
    source: &[u8],
    language: SupportedLanguage,
) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        let text = node_text(name, source);
        if !text.is_empty() {
            return Some(clean_symbol_name(&text));
        }
    }
    if matches!(
        language,
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript | SupportedLanguage::Tsx
    ) && matches!(node.kind(), "lexical_declaration" | "variable_declaration")
    {
        return first_child_text_by_kind(
            node,
            source,
            &["identifier", "shorthand_property_identifier"],
        );
    }
    if matches!(language, SupportedLanguage::Go) && node.kind() == "type_declaration" {
        return first_child_text_by_kind(node, source, &["type_identifier"]);
    }
    if matches!(language, SupportedLanguage::C | SupportedLanguage::Cpp)
        && node.kind() == "function_definition"
    {
        return deepest_identifier(node, source);
    }
    first_child_text_by_kind(
        node,
        source,
        &[
            "identifier",
            "type_identifier",
            "field_identifier",
            "property_identifier",
        ],
    )
}

fn extract_call_name(
    node: TsNode<'_>,
    source: &[u8],
    language: SupportedLanguage,
) -> Option<String> {
    for field in ["function", "name", "constructor"] {
        if let Some(child) = node.child_by_field_name(field) {
            let text = node_text(child, source);
            let cleaned = clean_callee_name(&text);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    if language == SupportedLanguage::Java {
        return first_child_text_by_kind(node, source, &["identifier"])
            .map(|text| clean_callee_name(&text));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            let text = node_text(child, source);
            let cleaned = clean_callee_name(&text);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

fn import_specifiers(text: &str, language: SupportedLanguage) -> Vec<String> {
    let quoted = quoted_strings(text);
    if !quoted.is_empty() {
        return quoted;
    }
    let trimmed = text.trim();
    match language {
        SupportedLanguage::Python => {
            if let Some(rest) = trimmed.strip_prefix("from ") {
                return rest
                    .split(" import ")
                    .next()
                    .map(clean_import_specifier)
                    .into_iter()
                    .collect();
            }
            if let Some(rest) = trimmed.strip_prefix("import ") {
                return rest
                    .split(',')
                    .map(|part| part.split_whitespace().next().unwrap_or_default())
                    .map(clean_import_specifier)
                    .filter(|part| !part.is_empty())
                    .collect();
            }
        }
        SupportedLanguage::Rust => {
            if let Some(rest) = trimmed.strip_prefix("use ") {
                return vec![clean_import_specifier(rest.trim_end_matches(';'))];
            }
            if let Some(rest) = trimmed.strip_prefix("mod ") {
                return vec![clean_import_specifier(rest.trim_end_matches(';'))];
            }
        }
        SupportedLanguage::Java => {
            if let Some(rest) = trimmed.strip_prefix("import ") {
                return vec![clean_import_specifier(
                    rest.trim_start_matches("static ").trim_end_matches(';'),
                )];
            }
        }
        _ => {}
    }
    Vec::new()
}

fn declaration_contains_function(node: TsNode<'_>, source: &[u8]) -> bool {
    let text = node_text(node, source);
    text.contains("=>") || text.contains("function")
}

fn node_text(node: TsNode<'_>, source: &[u8]) -> String {
    node.utf8_text(source)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn first_child_text_by_kind(node: TsNode<'_>, source: &[u8], kinds: &[&str]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() && kinds.contains(&child.kind()) {
            let text = node_text(child, source);
            if !text.is_empty() {
                return Some(clean_symbol_name(&text));
            }
        }
        if let Some(text) = first_child_text_by_kind(child, source, kinds) {
            return Some(text);
        }
    }
    None
}

fn deepest_identifier(node: TsNode<'_>, source: &[u8]) -> Option<String> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            if matches!(
                child.kind(),
                "identifier" | "field_identifier" | "type_identifier" | "property_identifier"
            ) {
                found = Some(clean_symbol_name(&node_text(child, source)));
            }
            if let Some(nested) = deepest_identifier(child, source) {
                found = Some(nested);
            }
        }
    }
    found
}

fn scoped_name(class_stack: &[String], name: &str) -> String {
    if class_stack.is_empty() {
        name.to_string()
    } else {
        let scope = class_stack
            .iter()
            .last()
            .map(|id| id.rsplit('_').next().unwrap_or(id))
            .unwrap_or_default();
        format!("{scope}.{name}")
    }
}

fn clean_symbol_name(text: &str) -> String {
    text.trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .to_string()
}

fn clean_callee_name(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() || text.starts_with('"') || text.starts_with('\'') {
        return String::new();
    }
    let text = text
        .split('(')
        .next()
        .unwrap_or(text)
        .trim()
        .trim_end_matches('!');
    let last = text
        .rsplit("::")
        .next()
        .unwrap_or(text)
        .rsplit('.')
        .next()
        .unwrap_or(text)
        .rsplit("->")
        .next()
        .unwrap_or(text);
    clean_symbol_name(last)
}

fn clean_import_specifier(specifier: &str) -> String {
    specifier
        .trim()
        .trim_matches(';')
        .trim_matches(',')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('<')
        .trim_matches('>')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

fn quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch != '"' && ch != '\'' {
            continue;
        }
        let quote = ch;
        let mut escaped = false;
        for (end, next) in chars.by_ref() {
            if escaped {
                escaped = false;
                continue;
            }
            if next == '\\' {
                escaped = true;
                continue;
            }
            if next == quote {
                if end > start + 1 {
                    out.push(text[start + 1..end].to_string());
                }
                break;
            }
        }
    }
    dedupe_preserving_order(out)
}

fn dedupe_preserving_order(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

fn is_builtin_call(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "println"
            | "console"
            | "len"
            | "str"
            | "int"
            | "float"
            | "bool"
            | "list"
            | "dict"
            | "set"
            | "map"
            | "filter"
            | "String"
            | "Number"
            | "Boolean"
            | "Object"
            | "Array"
            | "Vec"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
    )
}

fn looks_like_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test")
        || lower.contains("/tests")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.rs")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn indexes_package_manifest_and_kubernetes_refs_in_none_mode() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"demo-app","version":"1.0.0","dependencies":{"left-pad":"1.3.0"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("deploy.yaml"),
            r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  template:
    spec:
      containers:
        - name: web
          envFrom:
            - configMapRef:
                name: app-config
"#,
        )
        .unwrap();

        let graph = build_graph(dir.path()).unwrap();

        assert!(graph.nodes.iter().any(|node| {
            node.node_type.as_deref() == Some("package") && node.label == "demo-app"
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.relation == "depends_on" && edge.context.as_deref() == Some("dependency")
        }));
        let deployment = graph
            .nodes
            .iter()
            .find(|node| node.label == "Deployment/web")
            .unwrap();
        let config_map = graph
            .nodes
            .iter()
            .find(|node| node.label == "ConfigMap/app-config")
            .unwrap();
        assert!(graph.edges.iter().any(|edge| {
            edge.source == deployment.id
                && edge.target == config_map.id
                && edge.relation == "references"
        }));
        assert!(graph.nodes.iter().all(|node| {
            node.metadata.get("extraction_mode").and_then(Value::as_str) == Some("none")
        }));
    }

    #[test]
    fn indexes_ansible_playbooks_roles_tasks_and_handlers() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("roles/web/tasks")).unwrap();
        fs::create_dir_all(dir.path().join("roles/web/handlers")).unwrap();
        fs::create_dir_all(dir.path().join("roles/web/meta")).unwrap();

        fs::write(
            dir.path().join("site.yml"),
            r#"
- name: Configure web
  hosts: web
  roles:
    - web
  tasks:
    - name: Include extra tasks
      include_tasks: extra.yml
    - name: Pull common role
      import_role:
        name: common
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("roles/web/tasks/main.yml"),
            r#"
- name: Render config
  ansible.builtin.template:
    src: app.conf.j2
    dest: /etc/app.conf
  notify: Restart web
- name: Import package tasks
  import_tasks: packages.yml
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("roles/web/handlers/main.yml"),
            r#"
- name: Restart web
  ansible.builtin.service:
    name: httpd
    state: restarted
  listen: restart services
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("roles/web/meta/main.yml"),
            r#"
dependencies:
  - role: common
"#,
        )
        .unwrap();

        let graph = build_graph(dir.path()).unwrap();
        let web_role = graph
            .nodes
            .iter()
            .find(|node| node.node_type.as_deref() == Some("ansible_role") && node.label == "web")
            .unwrap();
        let common_role = graph
            .nodes
            .iter()
            .find(|node| {
                node.node_type.as_deref() == Some("ansible_role") && node.label == "common"
            })
            .unwrap();

        assert!(graph.nodes.iter().any(|node| {
            node.node_type.as_deref() == Some("ansible_play") && node.label == "Configure web"
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.target == web_role.id
                && edge.relation == "uses_role"
                && edge.context.as_deref() == Some("ansible_roles")
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.source == web_role.id
                && edge.target == common_role.id
                && edge.relation == "depends_on"
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.relation == "includes" && edge.context.as_deref() == Some("ansible_task_file")
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.relation == "imports" && edge.context.as_deref() == Some("ansible_task_file")
        }));
        assert!(graph.edges.iter().any(|edge| edge.relation == "notifies"));
        assert!(graph.nodes.iter().any(|node| {
            node.source_file == "site.yml"
                && node.metadata.get("extractor").and_then(Value::as_str) == Some("ansible")
        }));
    }

    #[test]
    fn mcp_config_indexes_env_names_without_values() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{
  "mcpServers": {
    "fetch": {
      "command": "uvx",
      "args": ["mcp-server-fetch"],
      "env": {"OPENAI_API_KEY": "sk-secret-value"}
    }
  }
}"#,
        )
        .unwrap();

        let graph = build_graph(dir.path()).unwrap();
        let serialized = serde_json::to_string(&graph).unwrap();

        assert!(serialized.contains("OPENAI_API_KEY"));
        assert!(serialized.contains("mcp-server-fetch"));
        assert!(!serialized.contains("sk-secret-value"));
    }

    #[test]
    fn data_json_does_not_expand_into_key_nodes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("data.json"), r#"[{"a":1},{"b":2}]"#).unwrap();

        let graph = build_graph(dir.path()).unwrap();

        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.source_file == "data.json")
                .count(),
            1
        );
        let file_node = graph
            .nodes
            .iter()
            .find(|node| node.source_file == "data.json")
            .unwrap();
        assert_eq!(
            file_node
                .metadata
                .get("skipped_structure")
                .and_then(Value::as_str),
            Some("data_json")
        );
    }
}
