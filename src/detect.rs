use crate::graph::repo_relative_path;
use anyhow::Result;
use ignore::{DirEntry, WalkBuilder};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".ai/graphify-light",
    "target",
    "node_modules",
    "vendor",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    "coverage",
];

const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".pyi", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".go", ".rs", ".java", ".groovy",
    ".gradle", ".cpp", ".cc", ".cxx", ".c", ".h", ".hpp", ".hh", ".hxx", ".cu", ".cuh", ".metal",
    ".rb", ".swift", ".kt", ".kts", ".cs", ".scala", ".php", ".lua", ".luau", ".zig", ".ps1",
    ".psm1", ".psd1", ".ex", ".exs", ".m", ".mm", ".jl", ".vue", ".svelte", ".astro", ".dart",
    ".v", ".sv", ".svh", ".sql", ".r", ".f", ".F", ".f90", ".F90", ".f95", ".F95", ".f03", ".F03",
    ".f08", ".F08", ".pas", ".pp", ".dpr", ".dpk", ".lpr", ".inc", ".dfm", ".lfm", ".lpk", ".sh",
    ".bash", ".json", ".toml", ".yaml", ".yml", ".tf", ".tfvars", ".hcl", ".sln", ".slnx",
    ".csproj", ".fsproj", ".vbproj", ".xaml", ".razor", ".cshtml",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SupportedLanguage {
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Rust,
    Go,
    Java,
    C,
    Cpp,
}

#[derive(Debug, Clone)]
pub struct DetectedFile {
    pub path: PathBuf,
    pub rel_path: String,
    pub language_name: String,
    pub supported_language: Option<SupportedLanguage>,
}

pub fn collect_code_files(root: &Path) -> Result<Vec<DetectedFile>> {
    let mut files = Vec::new();
    let skip_dirs: BTreeSet<&str> = SKIP_DIRS.iter().copied().collect();

    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(move |entry| keep_entry(entry, &skip_dirs))
        .build();

    for item in walker {
        let entry = item?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !is_code_file(path) || is_sensitive(path) || is_too_large(path) {
            continue;
        }
        let rel_path = repo_relative_path(root, path);
        let (language_name, supported_language) = classify_language(path);
        files.push(DetectedFile {
            path: path.to_path_buf(),
            rel_path,
            language_name,
            supported_language,
        });
    }

    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}

fn keep_entry(entry: &DirEntry, skip_dirs: &BTreeSet<&str>) -> bool {
    let path = entry.path();
    if path.is_dir() {
        let rel = path.to_string_lossy().replace('\\', "/");
        if rel.ends_with(".ai/graphify-light") || rel.contains("/.ai/graphify-light") {
            return false;
        }
        if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
            return !skip_dirs.contains(name);
        }
    }
    true
}

fn is_code_file(path: &Path) -> bool {
    if let Some(ext) = extension_with_dot(path) {
        return CODE_EXTENSIONS.iter().any(|known| known == &ext);
    }
    has_code_shebang(path)
}

fn classify_language(path: &Path) -> (String, Option<SupportedLanguage>) {
    match extension_with_dot(path).as_deref() {
        Some(".py") | Some(".pyi") => ("python".to_string(), Some(SupportedLanguage::Python)),
        Some(".js") | Some(".jsx") | Some(".mjs") | Some(".cjs") => (
            "javascript".to_string(),
            Some(SupportedLanguage::JavaScript),
        ),
        Some(".ts") => (
            "typescript".to_string(),
            Some(SupportedLanguage::TypeScript),
        ),
        Some(".tsx") => ("tsx".to_string(), Some(SupportedLanguage::Tsx)),
        Some(".rs") => ("rust".to_string(), Some(SupportedLanguage::Rust)),
        Some(".go") => ("go".to_string(), Some(SupportedLanguage::Go)),
        Some(".java") => ("java".to_string(), Some(SupportedLanguage::Java)),
        Some(".c") | Some(".h") => ("c".to_string(), Some(SupportedLanguage::C)),
        Some(".cc") | Some(".cpp") | Some(".cxx") | Some(".hpp") | Some(".hh") | Some(".hxx") => {
            ("cpp".to_string(), Some(SupportedLanguage::Cpp))
        }
        Some(".sh") | Some(".bash") => ("shell".to_string(), None),
        Some(".rb") => ("ruby".to_string(), None),
        Some(ext) => (ext.trim_start_matches('.').to_ascii_lowercase(), None),
        None => ("script".to_string(), None),
    }
}

fn extension_with_dot(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if name.ends_with(".blade.php") {
        return Some(".blade.php".to_string());
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
}

fn has_code_shebang(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let first_line = bytes.split(|b| *b == b'\n').next().unwrap_or_default();
    if !first_line.starts_with(b"#!") {
        return false;
    }
    let line = String::from_utf8_lossy(first_line).to_ascii_lowercase();
    [
        "python", "python3", "node", "nodejs", "ruby", "bash", "sh", "zsh", "fish", "lua", "php",
        "julia", "rscript",
    ]
    .iter()
    .any(|interpreter| line.contains(interpreter))
}

fn is_sensitive(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with(".env") {
        return true;
    }
    if [".pem", ".key", ".p12", ".pfx", ".crt", ".der", ".netrc"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
    {
        return true;
    }
    if ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"]
        .iter()
        .any(|secret| name == *secret || name.starts_with(&format!("{secret}.")))
    {
        return true;
    }
    path.components().any(|part| {
        let value = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(
            value.as_str(),
            ".ssh" | ".gnupg" | ".aws" | ".gcloud" | "secrets" | ".secrets" | "credentials"
        )
    })
}

fn is_too_large(path: &Path) -> bool {
    path.metadata()
        .map(|metadata| metadata.len() > MAX_SOURCE_BYTES)
        .unwrap_or(true)
}
