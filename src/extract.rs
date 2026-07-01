use crate::detect::{collect_code_files, DetectedFile, SupportedLanguage};
use crate::graph::{
    file_id, import_id, line_location, symbol_id, Edge, Graph, GraphBuilder, Node,
    CONFIDENCE_EXTRACTED, CONFIDENCE_INFERRED,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
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
    let files = collect_code_files(root)?;
    let file_lookup = build_file_lookup(&files);
    let mut builder = GraphBuilder::default();
    let mut sidecars = ExtractionSidecars::default();

    for file in &files {
        let file_result = extract_file(file, &file_lookup)
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

fn extract_file(
    file: &DetectedFile,
    file_lookup: &BTreeMap<String, String>,
) -> Result<(Vec<Node>, Vec<Edge>, ExtractionSidecars)> {
    let bytes = std::fs::read(&file.path)?;
    if let Some(language) = file.supported_language {
        match extract_tree_sitter(file, language, &bytes, file_lookup) {
            Ok(result) => return Ok(result),
            Err(_) => return Ok(extract_heuristic(file, &bytes, file_lookup)),
        }
    }
    Ok(extract_heuristic(file, &bytes, file_lookup))
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
    node.metadata
        .insert("path".to_string(), json!(file.rel_path.clone()));
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
