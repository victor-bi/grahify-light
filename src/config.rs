use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = ".graphify-light.toml";

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMode {
    None,
    LocalModel,
    Llm,
}

impl ExtractionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LocalModel => "local_model",
            Self::Llm => "llm",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphifyConfig {
    pub defaults: DefaultConfig,
    pub local_model: LocalModelConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultConfig {
    pub mode: ExtractionMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalModelConfig {
    pub allow_download: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct EffectiveExtraction {
    pub requested_mode: ExtractionMode,
    pub actual_mode: ExtractionMode,
    pub degraded_reason: Option<String>,
}

impl Default for GraphifyConfig {
    fn default() -> Self {
        Self {
            defaults: DefaultConfig::default(),
            local_model: LocalModelConfig::default(),
            llm: LlmConfig::default(),
        }
    }
}

impl Default for DefaultConfig {
    fn default() -> Self {
        Self {
            mode: ExtractionMode::None,
        }
    }
}

impl Default for LocalModelConfig {
    fn default() -> Self {
        Self {
            allow_download: false,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: String::new(),
            model: String::new(),
        }
    }
}

impl Default for ExtractionMode {
    fn default() -> Self {
        Self::None
    }
}

impl GraphifyConfig {
    pub fn load(root: &Path) -> Result<Self> {
        let path = config_path(root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn effective_extraction(&self) -> EffectiveExtraction {
        match self.defaults.mode {
            ExtractionMode::None => EffectiveExtraction {
                requested_mode: ExtractionMode::None,
                actual_mode: ExtractionMode::None,
                degraded_reason: None,
            },
            ExtractionMode::LocalModel => EffectiveExtraction {
                requested_mode: ExtractionMode::LocalModel,
                actual_mode: ExtractionMode::None,
                degraded_reason: Some(if self.local_model.allow_download {
                    "local_model_extractors_not_implemented".to_string()
                } else {
                    "local_model_download_disabled".to_string()
                }),
            },
            ExtractionMode::Llm if self.llm.enabled => EffectiveExtraction {
                requested_mode: ExtractionMode::Llm,
                actual_mode: ExtractionMode::None,
                degraded_reason: Some("llm_extraction_not_implemented".to_string()),
            },
            ExtractionMode::Llm => EffectiveExtraction {
                requested_mode: ExtractionMode::Llm,
                actual_mode: ExtractionMode::None,
                degraded_reason: Some("llm_disabled".to_string()),
            },
        }
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_FILE_NAME)
}

pub fn init_config(root: &Path, force: bool) -> Result<PathBuf> {
    let path = config_path(root);
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite it",
            path.display()
        );
    }
    fs::write(&path, config_template())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn config_template() -> &'static str {
    r#"# graphify-light configuration.
# The default mode is token-free and uses only deterministic local extraction.
#
# Token-free registered code formats include:
# Python, JavaScript, JSX, TypeScript, TSX, Rust, Go, Java, C, C++, Ruby,
# C#, Kotlin, Scala, PHP, Swift, Lua/Luau, Zig, PowerShell, Elixir,
# Objective-C, Julia, Verilog/SystemVerilog, Fortran, Bash/Shell, Dart,
# Groovy/Gradle, Vue, Svelte, Astro, Pascal/Delphi, BYOND DM, SQL, R,
# .NET project files, Razor, XAML, and Apex.
#
# Token-free config and infrastructure formats include:
# Terraform/HCL, Ansible playbooks/roles, Kubernetes YAML, JSON/JSONC config,
# TOML config, YAML config, MCP configs, and package manifests such as package.json, pyproject.toml,
# go.mod, pom.xml, Cargo.toml, and composer.json.
#
# Token-free resource formats include:
# Markdown, HTML, TXT, RST, PDF metadata/text preview, Office docx/xlsx/pptx
# text preview, image dimensions/metadata, audio/video container metadata,
# zip/tar/gz archive listings, and unknown binary resource metadata.

[defaults]
# Available: "none", "local_model", "llm"
# Default: "none"
# "none" never downloads models, never calls an LLM, and never uses remote APIs.
# "local_model" may use locally installed OCR/speech/vision tools in future releases.
# "llm" is reserved for explicit semantic extraction and is disabled by default.
mode = "none"

[local_model]
# Default: false. When false, graphify-light may only use models already present locally.
# The current Rust implementation does not download or run local model extractors yet.
allow_download = false

[llm]
# Default: false. LLM extraction must be explicitly enabled before any LLM path can run.
# The current Rust implementation still fails closed and does not call an LLM.
enabled = false
provider = ""
model = ""
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_none() {
        let config = GraphifyConfig::default();
        let effective = config.effective_extraction();
        assert_eq!(effective.requested_mode, ExtractionMode::None);
        assert_eq!(effective.actual_mode, ExtractionMode::None);
        assert!(effective.degraded_reason.is_none());
    }

    #[test]
    fn template_mentions_every_mode() {
        let template = config_template();
        assert!(template.contains(r#"mode = "none""#));
        assert!(template.contains("local_model"));
        assert!(template.contains("llm"));
        assert!(template.contains("Terraform/HCL"));
        assert!(template.contains("Kubernetes YAML"));
        assert!(template.contains("MCP configs"));
        assert!(template.contains("PDF metadata"));
        assert!(template.contains("never calls an LLM"));
    }
}
