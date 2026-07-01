use crate::extract::build_graph;
use crate::graph::{read_graph, write_graph};
use crate::query;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

pub fn run_stdio(root: &Path) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                write_message(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"code": -32700, "message": format!("Parse error: {error}")}
                    }),
                )?;
                continue;
            }
        };
        if let Some(response) = handle_message(root, request) {
            write_message(&mut stdout, response)?;
        }
    }
    Ok(())
}

fn handle_message(root: &Path, message: Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let is_notification = id.is_none();

    match method {
        "initialize" => Some(success(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": "graphify-light",
                    "title": "graphify-light",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Use graphify-light tools to query .ai/graphify-light/graph.json before broad repository scanning."
            }),
        )),
        "notifications/initialized" => None,
        "ping" => Some(success(id, json!({}))),
        "tools/list" => Some(success(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Some(match execute_tool(root, name, &arguments) {
                Ok(result) => {
                    let text = serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| result.to_string());
                    success(
                        id,
                        json!({
                            "content": [{"type": "text", "text": text}],
                            "structuredContent": result,
                            "isError": false
                        }),
                    )
                }
                Err(error) => success(
                    id,
                    json!({
                        "content": [{"type": "text", "text": error.to_string()}],
                        "isError": true
                    }),
                ),
            })
        }
        _ if is_notification => None,
        _ => Some(error_response(
            id,
            -32601,
            format!("Method not found: {method}"),
        )),
    }
}

fn execute_tool(root: &Path, name: &str, arguments: &Value) -> Result<Value> {
    match name {
        "refresh_index" => {
            let graph = build_graph(root)?;
            let path = write_graph(root, &graph)?;
            Ok(json!({
                "graph_path": path.to_string_lossy(),
                "stats": graph.stats()
            }))
        }
        "get_graph_stats" => {
            let graph = read_graph(root)?;
            Ok(query::get_graph_stats(&graph))
        }
        "find_symbol" => {
            let graph = read_graph(root)?;
            Ok(query::find_symbol(
                &graph,
                &required_string(arguments, "name")?,
            ))
        }
        "get_callers" => {
            let graph = read_graph(root)?;
            Ok(query::get_callers(
                &graph,
                &required_string(arguments, "name")?,
            ))
        }
        "get_callees" => {
            let graph = read_graph(root)?;
            Ok(query::get_callees(
                &graph,
                &required_string(arguments, "name")?,
            ))
        }
        "get_file_symbols" => {
            let graph = read_graph(root)?;
            Ok(query::get_file_symbols(
                &graph,
                &required_string(arguments, "path")?,
            ))
        }
        "search_nodes" => {
            let graph = read_graph(root)?;
            Ok(query::search_nodes(
                &graph,
                &required_string(arguments, "text")?,
            ))
        }
        "get_related_files" => {
            let graph = read_graph(root)?;
            Ok(query::get_related_files(
                &graph,
                &required_string(arguments, "path")?,
            ))
        }
        "get_imports" => {
            let graph = read_graph(root)?;
            Ok(query::get_imports(
                &graph,
                &required_string(arguments, "path")?,
            ))
        }
        "get_exports" => {
            let graph = read_graph(root)?;
            Ok(query::get_exports(
                &graph,
                &required_string(arguments, "path")?,
            ))
        }
        _ => Err(anyhow!("unknown tool: {name}")),
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "find_symbol",
            "Find symbols by exact or partial name.",
            object_schema(&[("name", "string", "Symbol name to find.")]),
        ),
        tool(
            "get_callers",
            "Return callers of matching symbols.",
            object_schema(&[("name", "string", "Symbol name to trace.")]),
        ),
        tool(
            "get_callees",
            "Return callees of matching symbols.",
            object_schema(&[("name", "string", "Symbol name to trace.")]),
        ),
        tool(
            "get_file_symbols",
            "Return symbols defined in a file.",
            object_schema(&[("path", "string", "Repo-relative file path.")]),
        ),
        tool(
            "search_nodes",
            "Search graph nodes by text.",
            object_schema(&[("text", "string", "Search text.")]),
        ),
        tool(
            "get_related_files",
            "Return files related to a file by graph edges.",
            object_schema(&[("path", "string", "Repo-relative file path.")]),
        ),
        tool(
            "get_imports",
            "Return imports for a file.",
            object_schema(&[("path", "string", "Repo-relative file path.")]),
        ),
        tool(
            "get_exports",
            "Return exported or defined symbols for a file.",
            object_schema(&[("path", "string", "Repo-relative file path.")]),
        ),
        tool(
            "get_graph_stats",
            "Return graph node, edge, language, relation, and confidence counts.",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "refresh_index",
            "Rebuild .ai/graphify-light/graph.json for the current repository.",
            json!({"type": "object", "properties": {}}),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "title": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn object_schema(fields: &[(&str, &str, &str)]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, field_type, description) in fields {
        properties.insert(
            (*name).to_string(),
            json!({
                "type": field_type,
                "description": description
            }),
        );
        required.push(json!(name));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
}

fn required_string(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("missing required string argument: {key}"))
}

fn success(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result
    })
}

fn error_response(id: Option<Value>, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn write_message(stdout: &mut io::Stdout, message: Value) -> Result<()> {
    serde_json::to_writer(&mut *stdout, &message)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
