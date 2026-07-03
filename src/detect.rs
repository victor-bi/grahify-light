use crate::graph::repo_relative_path;
use anyhow::Result;
use ignore::{DirEntry, WalkBuilder};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RESOURCE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_UNKNOWN_BYTES: u64 = 10 * 1024 * 1024;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "graphify-out",
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

const MCP_CONFIG_NAMES: &[&str] = &[
    ".mcp.json",
    "mcp.json",
    "mcp_servers.json",
    "claude_desktop_config.json",
];

const PACKAGE_MANIFEST_NAMES: &[&str] = &[
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "cargo.toml",
    "composer.json",
    "apm.yml",
    "apm.yaml",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FileCategory {
    Code,
    Config,
    Infrastructure,
    Document,
    Paper,
    Image,
    Office,
    AudioVideo,
    Archive,
    Resource,
}

impl FileCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Config => "config",
            Self::Infrastructure => "infrastructure",
            Self::Document => "document",
            Self::Paper => "paper",
            Self::Image => "image",
            Self::Office => "office",
            Self::AudioVideo => "audio_video",
            Self::Archive => "archive",
            Self::Resource => "resource",
        }
    }

    pub fn graph_file_type(self) -> &'static str {
        match self {
            Self::Code | Self::Config | Self::Infrastructure => "code",
            Self::Document | Self::Paper | Self::Office => "document",
            Self::Image | Self::AudioVideo | Self::Archive | Self::Resource => "resource",
        }
    }
}

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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExtractorKind {
    TreeSitter(SupportedLanguage),
    HeuristicCode,
    Terraform,
    JsonConfig,
    TomlConfig,
    YamlConfig,
    PackageManifest,
    McpConfig,
    Markdown,
    Text,
    Html,
    Pdf,
    Office,
    Image,
    AudioVideo,
    Archive,
    ResourceMetadata,
}

impl ExtractorKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::TreeSitter(_) => "tree_sitter",
            Self::HeuristicCode => "heuristic_code",
            Self::Terraform => "terraform_hcl",
            Self::JsonConfig => "json_config",
            Self::TomlConfig => "toml_config",
            Self::YamlConfig => "yaml_config",
            Self::PackageManifest => "package_manifest",
            Self::McpConfig => "mcp_config",
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Office => "office",
            Self::Image => "image",
            Self::AudioVideo => "audio_video",
            Self::Archive => "archive",
            Self::ResourceMetadata => "resource_metadata",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DetectedFile {
    pub path: PathBuf,
    pub rel_path: String,
    pub language_name: String,
    pub file_category: FileCategory,
    pub extractor: ExtractorKind,
    pub supported_language: Option<SupportedLanguage>,
}

#[derive(Debug, Clone)]
struct Classification {
    language_name: String,
    file_category: FileCategory,
    extractor: ExtractorKind,
    supported_language: Option<SupportedLanguage>,
}

pub fn collect_indexable_files(root: &Path) -> Result<Vec<DetectedFile>> {
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
        if !path.is_file() || is_sensitive(path) {
            continue;
        }
        let classification = classify_file(path);
        if is_too_large(path, classification.file_category) {
            continue;
        }
        let rel_path = repo_relative_path(root, path);
        files.push(DetectedFile {
            path: path.to_path_buf(),
            rel_path,
            language_name: classification.language_name,
            file_category: classification.file_category,
            extractor: classification.extractor,
            supported_language: classification.supported_language,
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

fn classify_file(path: &Path) -> Classification {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let lower_name = file_name.to_ascii_lowercase();

    if MCP_CONFIG_NAMES.iter().any(|name| lower_name == *name) {
        return class("json", FileCategory::Config, ExtractorKind::McpConfig, None);
    }
    if PACKAGE_MANIFEST_NAMES
        .iter()
        .any(|name| lower_name == *name)
    {
        return class(
            package_manifest_language(&lower_name),
            FileCategory::Code,
            ExtractorKind::PackageManifest,
            None,
        );
    }
    if lower_name.ends_with(".blade.php") {
        return class(
            "php",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        );
    }

    if let Some(ext) = extension_with_dot(path) {
        return classify_extension(&ext);
    }

    if let Some(language) = shebang_language(path) {
        return class(
            language,
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        );
    }

    class(
        "resource",
        FileCategory::Resource,
        ExtractorKind::ResourceMetadata,
        None,
    )
}

fn classify_extension(ext: &str) -> Classification {
    match ext {
        ".py" | ".pyi" => class(
            "python",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::Python),
            Some(SupportedLanguage::Python),
        ),
        ".js" | ".jsx" | ".mjs" | ".cjs" => class(
            "javascript",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::JavaScript),
            Some(SupportedLanguage::JavaScript),
        ),
        ".ts" | ".mts" | ".cts" => class(
            "typescript",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::TypeScript),
            Some(SupportedLanguage::TypeScript),
        ),
        ".tsx" => class(
            "tsx",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::Tsx),
            Some(SupportedLanguage::Tsx),
        ),
        ".rs" => class(
            "rust",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::Rust),
            Some(SupportedLanguage::Rust),
        ),
        ".go" => class(
            "go",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::Go),
            Some(SupportedLanguage::Go),
        ),
        ".java" => class(
            "java",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::Java),
            Some(SupportedLanguage::Java),
        ),
        ".c" | ".h" => class(
            "c",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::C),
            Some(SupportedLanguage::C),
        ),
        ".cc" | ".cpp" | ".cxx" | ".hpp" | ".hh" | ".hxx" | ".cu" | ".cuh" | ".metal" => class(
            "cpp",
            FileCategory::Code,
            ExtractorKind::TreeSitter(SupportedLanguage::Cpp),
            Some(SupportedLanguage::Cpp),
        ),
        ".tf" | ".tfvars" | ".hcl" => class(
            "terraform",
            FileCategory::Infrastructure,
            ExtractorKind::Terraform,
            None,
        ),
        ".json" | ".jsonc" => class(
            "json",
            FileCategory::Config,
            ExtractorKind::JsonConfig,
            None,
        ),
        ".toml" => class(
            "toml",
            FileCategory::Config,
            ExtractorKind::TomlConfig,
            None,
        ),
        ".yaml" | ".yml" => class(
            "yaml",
            FileCategory::Config,
            ExtractorKind::YamlConfig,
            None,
        ),
        ".md" | ".mdx" | ".qmd" => class(
            "markdown",
            FileCategory::Document,
            ExtractorKind::Markdown,
            None,
        ),
        ".html" | ".htm" => class("html", FileCategory::Document, ExtractorKind::Html, None),
        ".txt" | ".rst" => class("text", FileCategory::Document, ExtractorKind::Text, None),
        ".pdf" => class("pdf", FileCategory::Paper, ExtractorKind::Pdf, None),
        ".docx" | ".xlsx" | ".pptx" => {
            class("office", FileCategory::Office, ExtractorKind::Office, None)
        }
        ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp" | ".svg" => {
            class("image", FileCategory::Image, ExtractorKind::Image, None)
        }
        ".mp4" | ".mov" | ".webm" | ".mkv" | ".avi" | ".m4v" | ".mp3" | ".wav" | ".m4a"
        | ".ogg" => class(
            "media",
            FileCategory::AudioVideo,
            ExtractorKind::AudioVideo,
            None,
        ),
        ".zip" | ".tar" | ".tgz" | ".tar.gz" | ".gz" => class(
            "archive",
            FileCategory::Archive,
            ExtractorKind::Archive,
            None,
        ),
        ".sh" | ".bash" => class(
            "shell",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".rb" => class(
            "ruby",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".cs" => class(
            "csharp",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".kt" | ".kts" => class(
            "kotlin",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".scala" => class(
            "scala",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".php" => class(
            "php",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".swift" => class(
            "swift",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".lua" | ".luau" | ".toc" => class(
            "lua",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".zig" => class(
            "zig",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".ps1" | ".psm1" | ".psd1" => class(
            "powershell",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".ex" | ".exs" => class(
            "elixir",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".m" | ".mm" => class(
            "objective_c",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".jl" => class(
            "julia",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".v" | ".sv" | ".svh" => class(
            "verilog",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".f" | ".F" | ".f90" | ".F90" | ".f95" | ".F95" | ".f03" | ".F03" | ".f08" | ".F08" => {
            class(
                "fortran",
                FileCategory::Code,
                ExtractorKind::HeuristicCode,
                None,
            )
        }
        ".dart" => class(
            "dart",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".groovy" | ".gradle" => class(
            "groovy",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".vue" => class(
            "vue",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".svelte" => class(
            "svelte",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".astro" => class(
            "astro",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".pas" | ".pp" | ".dpr" | ".dpk" | ".lpr" | ".inc" | ".dfm" | ".lfm" | ".lpk" => class(
            "pascal",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".dm" | ".dme" | ".dmi" | ".dmm" | ".dmf" => class(
            "byond_dm",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".sql" => class(
            "sql",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        ".r" => class("r", FileCategory::Code, ExtractorKind::HeuristicCode, None),
        ".sln" | ".slnx" | ".csproj" | ".fsproj" | ".vbproj" | ".xaml" | ".razor" | ".cshtml" => {
            class(
                "dotnet",
                FileCategory::Code,
                ExtractorKind::HeuristicCode,
                None,
            )
        }
        ".cls" | ".trigger" => class(
            "apex",
            FileCategory::Code,
            ExtractorKind::HeuristicCode,
            None,
        ),
        _ => class(
            ext.trim_start_matches('.'),
            FileCategory::Resource,
            ExtractorKind::ResourceMetadata,
            None,
        ),
    }
}

fn class(
    language_name: impl Into<String>,
    file_category: FileCategory,
    extractor: ExtractorKind,
    supported_language: Option<SupportedLanguage>,
) -> Classification {
    Classification {
        language_name: language_name.into(),
        file_category,
        extractor,
        supported_language,
    }
}

fn package_manifest_language(name: &str) -> &'static str {
    match name {
        "package.json" => "npm",
        "pyproject.toml" => "python",
        "go.mod" => "go",
        "pom.xml" => "maven",
        "cargo.toml" => "rust",
        "composer.json" => "composer",
        "apm.yml" | "apm.yaml" => "apm",
        _ => "package_manifest",
    }
}

fn extension_with_dot(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if name.ends_with(".blade.php") {
        return Some(".blade.php".to_string());
    }
    if name.ends_with(".tar.gz") {
        return Some(".tar.gz".to_string());
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
}

fn shebang_language(path: &Path) -> Option<&'static str> {
    let Ok(bytes) = std::fs::read(path) else {
        return None;
    };
    let first_line = bytes.split(|b| *b == b'\n').next().unwrap_or_default();
    if !first_line.starts_with(b"#!") {
        return None;
    }
    let line = String::from_utf8_lossy(first_line).to_ascii_lowercase();
    for (needle, language) in [
        ("python", "python"),
        ("python3", "python"),
        ("node", "javascript"),
        ("nodejs", "javascript"),
        ("ruby", "ruby"),
        ("bash", "shell"),
        ("sh", "shell"),
        ("zsh", "shell"),
        ("fish", "shell"),
        ("lua", "lua"),
        ("php", "php"),
        ("julia", "julia"),
        ("rscript", "r"),
    ] {
        if line.contains(needle) {
            return Some(language);
        }
    }
    None
}

fn is_sensitive(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with(".env") || name == ".netrc" || name == ".pgpass" || name == ".htpasswd" {
        return true;
    }
    if [
        ".pem", ".key", ".p12", ".pfx", ".cert", ".crt", ".der", ".p8",
    ]
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
    if generic_secret_keyword_hit(&name) {
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

fn generic_secret_keyword_hit(name: &str) -> bool {
    let stem = name
        .trim_start_matches('.')
        .split('.')
        .next()
        .unwrap_or(name);
    let parts = stem
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let sensitive = [
        "credential",
        "credentials",
        "secret",
        "secrets",
        "passwd",
        "password",
        "private_key",
        "token",
        "tokens",
    ];
    sensitive.iter().any(|keyword| {
        stem == *keyword
            || stem.ends_with(&format!("_{keyword}"))
            || stem.ends_with(&format!("-{keyword}"))
            || (parts.len() <= 2 && parts.iter().any(|part| part == keyword))
    })
}

fn is_too_large(path: &Path, category: FileCategory) -> bool {
    let cap = match category {
        FileCategory::Code | FileCategory::Config | FileCategory::Infrastructure => {
            MAX_SOURCE_BYTES
        }
        FileCategory::Resource => MAX_UNKNOWN_BYTES,
        _ => MAX_RESOURCE_BYTES,
    };
    path.metadata()
        .map(|metadata| metadata.len() > cap)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_special_configs_by_name() {
        let mcp = classify_file(Path::new(".mcp.json"));
        assert_eq!(mcp.extractor, ExtractorKind::McpConfig);

        let package = classify_file(Path::new("Cargo.toml"));
        assert_eq!(package.extractor, ExtractorKind::PackageManifest);
        assert_eq!(package.language_name, "rust");
    }

    #[test]
    fn unknown_files_are_resources() {
        let file = classify_file(Path::new("diagram.bin"));
        assert_eq!(file.file_category, FileCategory::Resource);
        assert_eq!(file.extractor, ExtractorKind::ResourceMetadata);
    }
}
