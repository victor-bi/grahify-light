use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wait_timeout::ChildExt;

#[derive(Debug, Parser)]
#[command(name = "graphify-bench")]
#[command(about = "Run a containerized Codex token benchmark for repository understanding.")]
struct Cli {
    #[arg(long, default_value = "/workspace/source")]
    repo: PathBuf,

    #[arg(long, default_value = "/workspace/out")]
    out: PathBuf,

    #[arg(long, default_value = "codex")]
    codex_bin: String,

    #[arg(long)]
    model: Option<String>,

    #[arg(long, default_value = "/opt/graphify/bin/graphify")]
    graphify_bin: String,

    #[arg(long, default_value = "graphify-light")]
    graphify_light_bin: String,

    #[arg(long, default_value_t = 900)]
    timeout_secs: u64,

    #[arg(long)]
    allow_host: bool,

    #[arg(long)]
    allow_failures: bool,
}

#[derive(Debug, Clone, Copy)]
struct Variant {
    id: &'static str,
    label: &'static str,
    context: &'static str,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
struct TokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
}

impl TokenUsage {
    fn total(self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, Copy)]
struct BenchmarkRound {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct InitBaseline {
    total_tokens: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    codex_seconds: f64,
}

#[derive(Debug, Serialize)]
struct BenchmarkResult {
    round: String,
    round_description: String,
    variant: String,
    context: String,
    status: String,
    total_tokens: Option<u64>,
    init_total_tokens: Option<u64>,
    task_tokens: Option<u64>,
    input_tokens: Option<u64>,
    init_input_tokens: Option<u64>,
    task_input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    token_savings_vs_direct_percent: Option<f64>,
    prep_seconds: f64,
    codex_seconds: f64,
    end_to_end_seconds: f64,
    codex_seconds_saved_vs_direct: Option<f64>,
    codex_time_savings_vs_direct_percent: Option<f64>,
    end_to_end_seconds_saved_vs_direct: Option<f64>,
    end_to_end_time_savings_vs_direct_percent: Option<f64>,
    context_path: Option<String>,
    answer_path: Option<String>,
    log_path: Option<String>,
    error: Option<String>,
}

#[derive(Debug)]
struct CommandResult {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    seconds: f64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if !cli.allow_host && !running_in_container() {
        bail!(
            "this benchmark is intended to run inside a container; use benchmarking/Dockerfile or pass --allow-host for local debugging"
        );
    }

    let repo = cli
        .repo
        .canonicalize()
        .with_context(|| format!("failed to resolve repo path {}", cli.repo.display()))?;
    let out = absolutize(&cli.out)?;

    fs::create_dir_all(&out).with_context(|| format!("failed to create {}", out.display()))?;
    let logs_dir = out.join("logs");
    let answers_dir = out.join("answers");
    let work_dir = out.join("work");
    recreate_dir(&logs_dir)?;
    recreate_dir(&answers_dir)?;
    recreate_dir(&work_dir)?;

    let timeout = Duration::from_secs(cli.timeout_secs);
    let init_baseline = run_init_baseline(&cli, &work_dir, &logs_dir, &answers_dir, timeout)?;
    let variants = [
        Variant {
            id: "direct-codex",
            label: "Direct Codex",
            context: "Raw repository files",
        },
        Variant {
            id: "graphify",
            label: "Graphify",
            context: "Graphify graphify-out artifacts",
        },
        Variant {
            id: "graphify-light",
            label: "Graphify Light",
            context: "graphify-light graph.json",
        },
    ];
    let rounds = [
        BenchmarkRound {
            id: "round-1-understanding",
            label: "Round 1: repository understanding",
            description: "Ask Codex to produce a concise repository-understanding report.",
        },
        BenchmarkRound {
            id: "round-2-follow-up",
            label: "Round 2: follow-up question",
            description: "Ask Codex a second, narrower architecture question against the same prepared context.",
        },
    ];

    let mut results = Vec::new();
    for variant in variants {
        eprintln!("preparing {}...", variant.label);
        match prepare_variant(&cli, variant, &repo, &work_dir, &logs_dir, timeout) {
            Ok(prepared) => {
                for round in rounds {
                    eprintln!("running {} / {}...", round.label, variant.label);
                    let result = run_variant_round(
                        &cli,
                        &prepared,
                        round,
                        &init_baseline,
                        &logs_dir,
                        &answers_dir,
                        timeout,
                    );
                    match result {
                        Ok(result) => results.push(result),
                        Err(error) => results.push(failed_result(
                            round,
                            variant,
                            prepared.prep_seconds,
                            Some(prepared.context_dir.clone()),
                            error,
                        )),
                    }
                }
            }
            Err(error) => {
                let error = format!("failed to prepare {}: {error}", variant.label);
                for round in rounds {
                    results.push(failed_result(
                        round,
                        variant,
                        0.0,
                        None,
                        anyhow!(error.clone()),
                    ));
                }
            }
        }
    }

    apply_comparisons(&mut results);

    let markdown = render_markdown_report(&init_baseline, &rounds, &results);
    fs::write(out.join("results.md"), &markdown)
        .with_context(|| format!("failed to write {}", out.join("results.md").display()))?;
    fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "init_baseline": init_baseline,
            "results": results
        }))?,
    )
    .with_context(|| format!("failed to write {}", out.join("results.json").display()))?;
    write_round_tables(&out, &init_baseline, &rounds, &results)?;

    println!("{markdown}");

    if !cli.allow_failures && results.iter().any(|result| result.status != "ok") {
        bail!(
            "one or more benchmark variants failed; see {}",
            out.display()
        );
    }

    Ok(())
}

#[derive(Debug)]
struct PreparedVariant {
    variant: Variant,
    context_dir: PathBuf,
    prep_seconds: f64,
}

fn run_init_baseline(
    cli: &Cli,
    work_dir: &Path,
    logs_dir: &Path,
    answers_dir: &Path,
    timeout: Duration,
) -> Result<InitBaseline> {
    let init_dir = work_dir.join("codex-init-baseline");
    recreate_dir(&init_dir)?;
    fs::write(
        init_dir.join("README.md"),
        "Empty workspace used only to measure Codex initialization overhead.\n",
    )?;

    let answer_path = answers_dir.join("codex-init-baseline.md");
    let log_path = logs_dir.join("codex-init-baseline.jsonl");
    let prompt = "Return exactly OK. Do not inspect files.";
    let codex = run_codex(cli, &init_dir, &answer_path, prompt, timeout)
        .context("failed to run Codex initialization baseline")?;
    let combined = combine_streams(&codex.stdout, &codex.stderr);
    fs::write(&log_path, &combined)
        .with_context(|| format!("failed to write {}", log_path.display()))?;

    if !codex.status.success() {
        bail!(
            "Codex initialization baseline exited with status {}; see {}",
            codex.status,
            log_path.display()
        );
    }

    let usage = parse_usage(&combined).with_context(|| {
        format!(
            "failed to parse Codex initialization token usage from {}",
            log_path.display()
        )
    })?;

    Ok(InitBaseline {
        total_tokens: usage.total(),
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        codex_seconds: codex.seconds,
    })
}

fn prepare_variant(
    cli: &Cli,
    variant: Variant,
    repo: &Path,
    work_dir: &Path,
    logs_dir: &Path,
    timeout: Duration,
) -> Result<PreparedVariant> {
    let variant_root = work_dir.join(variant.id);
    let corpus = variant_root.join("repo");
    recreate_dir(&variant_root)?;
    copy_repo(repo, &corpus)?;

    let prep_start = Instant::now();
    let context_dir = match variant.id {
        "direct-codex" => corpus.clone(),
        "graphify" => {
            let graphify = run_command(
                &cli.graphify_bin,
                vec![
                    OsString::from("update"),
                    OsString::from("."),
                    OsString::from("--no-cluster"),
                ],
                &corpus,
                timeout,
            )
            .context("failed to run Graphify")?;
            write_command_log(&logs_dir.join("graphify-build.log"), &graphify)?;
            if !graphify.status.success() {
                bail!(
                    "Graphify exited with status {}; see {}",
                    graphify.status,
                    logs_dir.join("graphify-build.log").display()
                );
            }
            let context = corpus.join("graphify-out");
            if !context.join("graph.json").exists() && !context.join("GRAPH_REPORT.md").exists() {
                bail!("Graphify did not produce {}", context.display());
            }
            context
        }
        "graphify-light" => {
            let graphify_light = run_command(
                &cli.graphify_light_bin,
                vec![OsString::from("build")],
                &corpus,
                timeout,
            )
            .context("failed to run graphify-light")?;
            write_command_log(&logs_dir.join("graphify-light-build.log"), &graphify_light)?;
            if !graphify_light.status.success() {
                bail!(
                    "graphify-light exited with status {}; see {}",
                    graphify_light.status,
                    logs_dir.join("graphify-light-build.log").display()
                );
            }
            let context = corpus.join(".ai").join("graphify-light");
            if !context.join("graph.json").exists() {
                bail!(
                    "graphify-light did not produce {}",
                    context.join("graph.json").display()
                );
            }
            context
        }
        _ => unreachable!("unknown variant"),
    };
    let prep_seconds = prep_start.elapsed().as_secs_f64();

    Ok(PreparedVariant {
        variant,
        context_dir,
        prep_seconds,
    })
}

fn run_variant_round(
    cli: &Cli,
    prepared: &PreparedVariant,
    round: BenchmarkRound,
    init_baseline: &InitBaseline,
    logs_dir: &Path,
    answers_dir: &Path,
    timeout: Duration,
) -> Result<BenchmarkResult> {
    let variant = prepared.variant;
    let answer_path = answers_dir.join(format!("{}-{}.md", variant.id, round.id));
    let codex_log_path = logs_dir.join(format!("{}-{}.jsonl", variant.id, round.id));
    let prompt = prompt_for_variant_round(variant, round);
    let codex = run_codex(cli, &prepared.context_dir, &answer_path, &prompt, timeout)
        .with_context(|| format!("failed to run Codex for {}", variant.label))?;

    let combined = combine_streams(&codex.stdout, &codex.stderr);
    fs::write(&codex_log_path, &combined)
        .with_context(|| format!("failed to write {}", codex_log_path.display()))?;

    if !codex.status.success() {
        bail!(
            "Codex exited with status {}; see {}",
            codex.status,
            codex_log_path.display()
        );
    }

    let usage = parse_usage(&combined).with_context(|| {
        format!(
            "failed to parse Codex token usage from {}",
            codex_log_path.display()
        )
    })?;
    let task_total = usage.total().saturating_sub(init_baseline.total_tokens);
    let task_input = usage
        .input_tokens
        .saturating_sub(init_baseline.input_tokens);
    let end_to_end_seconds = prepared.prep_seconds + codex.seconds;

    Ok(BenchmarkResult {
        round: round.label.to_string(),
        round_description: round.description.to_string(),
        variant: variant.label.to_string(),
        context: variant.context.to_string(),
        status: "ok".to_string(),
        total_tokens: Some(usage.total()),
        init_total_tokens: Some(init_baseline.total_tokens),
        task_tokens: Some(task_total),
        input_tokens: Some(usage.input_tokens),
        init_input_tokens: Some(init_baseline.input_tokens),
        task_input_tokens: Some(task_input),
        cached_input_tokens: Some(usage.cached_input_tokens),
        output_tokens: Some(usage.output_tokens),
        reasoning_output_tokens: Some(usage.reasoning_output_tokens),
        token_savings_vs_direct_percent: None,
        prep_seconds: prepared.prep_seconds,
        codex_seconds: codex.seconds,
        end_to_end_seconds,
        codex_seconds_saved_vs_direct: None,
        codex_time_savings_vs_direct_percent: None,
        end_to_end_seconds_saved_vs_direct: None,
        end_to_end_time_savings_vs_direct_percent: None,
        context_path: Some(path_string(prepared.context_dir.clone())),
        answer_path: Some(path_string(answer_path)),
        log_path: Some(path_string(codex_log_path)),
        error: None,
    })
}

fn run_codex(
    cli: &Cli,
    context_dir: &Path,
    answer_path: &Path,
    prompt: &str,
    timeout: Duration,
) -> Result<CommandResult> {
    let mut args = vec![
        OsString::from("exec"),
        OsString::from("--json"),
        OsString::from("--color"),
        OsString::from("never"),
        OsString::from("--ephemeral"),
        OsString::from("--ignore-user-config"),
        OsString::from("--ignore-rules"),
        OsString::from("--skip-git-repo-check"),
        OsString::from("--sandbox"),
        OsString::from("danger-full-access"),
        OsString::from("-C"),
        context_dir.as_os_str().to_os_string(),
        OsString::from("-o"),
        answer_path.as_os_str().to_os_string(),
    ];

    if let Some(model) = &cli.model {
        args.push(OsString::from("--model"));
        args.push(OsString::from(model));
    }

    args.push(OsString::from(prompt));
    run_command(&cli.codex_bin, args, context_dir, timeout)
}

fn prompt_for_variant_round(variant: Variant, round: BenchmarkRound) -> String {
    let task = match round.id {
        "round-1-understanding" => {
            "Write a concise repository-understanding report. Include purpose, architecture, main modules, CLI/build/test commands, generated artifacts, and notable integration points. Keep the answer under 800 words."
        }
        "round-2-follow-up" => {
            "Answer this follow-up architecture question: where are CLI dispatch, graph construction, query handling, MCP handling, and Codex installation integration represented? Include key files and symbols if inferable. Keep the answer under 500 words."
        }
        _ => unreachable!("unknown round"),
    };

    match variant.id {
        "direct-codex" => format!(
            "You are benchmarking Codex with raw repository files. Inspect this repository as needed. Do not modify files. {task}"
        ),
        "graphify" => format!(
            "You are benchmarking Codex with Graphify output. Use only the files in the current graphify-out directory, especially graph.json and any report files if present. Do not inspect source files outside this directory. Do not modify files. {task}"
        ),
        "graphify-light" => format!(
            "You are benchmarking Codex with graphify-light output. Use only the files in the current .ai/graphify-light directory, especially graph.json. Do not inspect source files outside this directory. Do not modify files. {task}"
        ),
        _ => unreachable!("unknown variant"),
    }
}

fn run_command(
    program: &str,
    args: impl IntoIterator<Item = OsString>,
    cwd: &Path,
    timeout: Duration,
) -> Result<CommandResult> {
    let args: Vec<OsString> = args.into_iter().collect();
    let stdout_path = temp_output_path(program, "stdout");
    let stderr_path = temp_output_path(program, "stderr");
    let stdout_file = fs::File::create(&stdout_path)
        .with_context(|| format!("failed to create {}", stdout_path.display()))?;
    let stderr_file = fs::File::create(&stderr_path)
        .with_context(|| format!("failed to create {}", stderr_path.display()))?;
    let start = Instant::now();
    let mut child = Command::new(program)
        .args(&args)
        .current_dir(cwd)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .with_context(|| format!("failed to spawn {program}"))?;

    let status = match child.wait_timeout(timeout)? {
        Some(status) => status,
        None => {
            child.kill().ok();
            child.wait().ok();
            let stdout = read_lossy(&stdout_path);
            let stderr = read_lossy(&stderr_path);
            cleanup_temp_outputs(&stdout_path, &stderr_path);
            bail!(
                "{program} timed out after {} seconds\nstdout:\n{}\nstderr:\n{}",
                timeout.as_secs(),
                tail(&stdout, 2_000),
                tail(&stderr, 2_000)
            );
        }
    };

    let stdout = read_lossy(&stdout_path);
    let stderr = read_lossy(&stderr_path);
    cleanup_temp_outputs(&stdout_path, &stderr_path);

    Ok(CommandResult {
        status,
        stdout,
        stderr,
        seconds: start.elapsed().as_secs_f64(),
    })
}

fn temp_output_path(program: &str, stream: &str) -> PathBuf {
    let safe_program: String = program
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "graphify-bench-{}-{nanos}-{safe_program}-{stream}.log",
        std::process::id()
    ))
}

fn read_lossy(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default()
}

fn cleanup_temp_outputs(stdout_path: &Path, stderr_path: &Path) {
    fs::remove_file(stdout_path).ok();
    fs::remove_file(stderr_path).ok();
}

fn tail(value: &str, max_chars: usize) -> String {
    let len = value.chars().count();
    if len <= max_chars {
        return value.to_string();
    }
    value.chars().skip(len - max_chars).collect()
}

fn parse_usage(log: &str) -> Result<TokenUsage> {
    let mut usage = TokenUsage::default();
    let mut found = false;

    for line in log.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        if value.get("type").and_then(Value::as_str) != Some("turn.completed") {
            continue;
        }

        let Some(event_usage) = value.get("usage") else {
            continue;
        };

        usage.input_tokens += json_u64(event_usage, "input_tokens");
        usage.cached_input_tokens += json_u64(event_usage, "cached_input_tokens");
        usage.output_tokens += json_u64(event_usage, "output_tokens");
        usage.reasoning_output_tokens += json_u64(event_usage, "reasoning_output_tokens");
        found = true;
    }

    found
        .then_some(usage)
        .ok_or_else(|| anyhow!("no turn.completed usage event found"))
}

fn json_u64(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn failed_result(
    round: BenchmarkRound,
    variant: Variant,
    prep_seconds: f64,
    context_dir: Option<PathBuf>,
    error: anyhow::Error,
) -> BenchmarkResult {
    BenchmarkResult {
        round: round.label.to_string(),
        round_description: round.description.to_string(),
        variant: variant.label.to_string(),
        context: variant.context.to_string(),
        status: "failed".to_string(),
        total_tokens: None,
        init_total_tokens: None,
        task_tokens: None,
        input_tokens: None,
        init_input_tokens: None,
        task_input_tokens: None,
        cached_input_tokens: None,
        output_tokens: None,
        reasoning_output_tokens: None,
        token_savings_vs_direct_percent: None,
        prep_seconds,
        codex_seconds: 0.0,
        end_to_end_seconds: prep_seconds,
        codex_seconds_saved_vs_direct: None,
        codex_time_savings_vs_direct_percent: None,
        end_to_end_seconds_saved_vs_direct: None,
        end_to_end_time_savings_vs_direct_percent: None,
        context_path: context_dir.map(path_string),
        answer_path: None,
        log_path: None,
        error: Some(error.to_string()),
    }
}

fn apply_comparisons(results: &mut [BenchmarkResult]) {
    let rounds: Vec<String> = results.iter().map(|result| result.round.clone()).collect();
    for round in rounds {
        let Some(baseline) = results
            .iter()
            .find(|result| result.round == round && result.variant == "Direct Codex")
        else {
            continue;
        };

        let baseline_task_tokens = baseline.task_tokens.filter(|tokens| *tokens > 0);
        let baseline_codex_seconds =
            (baseline.codex_seconds > 0.0).then_some(baseline.codex_seconds);
        let baseline_end_to_end_seconds =
            (baseline.end_to_end_seconds > 0.0).then_some(baseline.end_to_end_seconds);

        for result in results.iter_mut().filter(|result| result.round == round) {
            result.token_savings_vs_direct_percent =
                percent_saved_u64(baseline_task_tokens, result.task_tokens);
            result.codex_seconds_saved_vs_direct =
                seconds_saved(baseline_codex_seconds, result.codex_seconds);
            result.codex_time_savings_vs_direct_percent =
                percent_saved_f64(baseline_codex_seconds, result.codex_seconds);
            result.end_to_end_seconds_saved_vs_direct =
                seconds_saved(baseline_end_to_end_seconds, result.end_to_end_seconds);
            result.end_to_end_time_savings_vs_direct_percent =
                percent_saved_f64(baseline_end_to_end_seconds, result.end_to_end_seconds);
        }
    }
}

fn render_markdown_report(
    init_baseline: &InitBaseline,
    rounds: &[BenchmarkRound],
    results: &[BenchmarkResult],
) -> String {
    let mut output = String::new();
    output.push_str("# Benchmark Results\n\n");
    output.push_str("Codex initialization baseline, measured with a no-op prompt and subtracted from task-token comparisons:\n\n");
    output.push_str("| Init total tokens | Init input tokens | Init cached input | Init output tokens | Init reasoning tokens | Init Codex seconds |\n");
    output.push_str("|---:|---:|---:|---:|---:|---:|\n");
    output.push_str(&format!(
        "| {} | {} | {} | {} | {} | {:.2} |\n\n",
        init_baseline.total_tokens,
        init_baseline.input_tokens,
        init_baseline.cached_input_tokens,
        init_baseline.output_tokens,
        init_baseline.reasoning_output_tokens,
        init_baseline.codex_seconds
    ));

    for round in rounds {
        output.push_str(&format!("## {}\n\n{}\n\n", round.label, round.description));
        output.push_str(&render_round_table(round, results));
        output.push('\n');
    }

    let failures: Vec<_> = results
        .iter()
        .filter_map(|result| {
            result
                .error
                .as_ref()
                .map(|error| (&result.round, &result.variant, error))
        })
        .collect();
    if !failures.is_empty() {
        output.push_str("## Failures\n\n");
        for (round, variant, error) in failures {
            output.push_str(&format!("- {round} / {variant}: {error}\n"));
        }
    }

    output
}

fn render_round_table(round: &BenchmarkRound, results: &[BenchmarkResult]) -> String {
    let mut output = String::from(
        "| Variant | Status | Task tokens excl. init | Token savings vs Direct | Tokens saved | Local prep seconds | Codex seconds | Codex seconds saved | Codex time saved | End-to-end seconds | End-to-end seconds saved | End-to-end time saved |\n",
    );
    output.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");

    let baseline_task_tokens = results
        .iter()
        .find(|result| result.round == round.label && result.variant == "Direct Codex")
        .and_then(|result| result.task_tokens);

    for result in results.iter().filter(|result| result.round == round.label) {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.2} | {:.2} | {} | {} | {:.2} | {} | {} |\n",
            result.variant,
            result.status,
            fmt_u64(result.task_tokens),
            fmt_percent(result.token_savings_vs_direct_percent),
            fmt_saved_tokens(baseline_task_tokens, result.task_tokens),
            result.prep_seconds,
            result.codex_seconds,
            fmt_seconds(result.codex_seconds_saved_vs_direct),
            fmt_percent(result.codex_time_savings_vs_direct_percent),
            result.end_to_end_seconds,
            fmt_seconds(result.end_to_end_seconds_saved_vs_direct),
            fmt_percent(result.end_to_end_time_savings_vs_direct_percent),
        ));
    }

    output
}

fn write_round_tables(
    out: &Path,
    init_baseline: &InitBaseline,
    rounds: &[BenchmarkRound],
    results: &[BenchmarkResult],
) -> Result<()> {
    for round in rounds {
        let mut markdown = String::new();
        markdown.push_str(&format!("# {}\n\n{}\n\n", round.label, round.description));
        markdown.push_str(&format!(
            "Init baseline: {} total tokens, {} input tokens, {:.2} Codex seconds.\n\n",
            init_baseline.total_tokens, init_baseline.input_tokens, init_baseline.codex_seconds
        ));
        markdown.push_str(&render_round_table(round, results));
        fs::write(out.join(format!("{}.md", round.id)), markdown).with_context(|| {
            format!(
                "failed to write {}",
                out.join(format!("{}.md", round.id)).display()
            )
        })?;
    }
    Ok(())
}

fn percent_saved_u64(baseline: Option<u64>, value: Option<u64>) -> Option<f64> {
    let baseline = baseline?;
    let value = value?;
    (baseline > 0).then_some(((baseline as f64 - value as f64) / baseline as f64) * 100.0)
}

fn percent_saved_f64(baseline: Option<f64>, value: f64) -> Option<f64> {
    let baseline = baseline?;
    (baseline > 0.0).then_some(((baseline - value) / baseline) * 100.0)
}

fn seconds_saved(baseline: Option<f64>, value: f64) -> Option<f64> {
    baseline.map(|baseline| baseline - value)
}

fn fmt_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn fmt_saved_tokens(baseline: Option<u64>, value: Option<u64>) -> String {
    match (baseline, value) {
        (Some(baseline), Some(value)) => {
            let saved = baseline as i128 - value as i128;
            format_signed_i128(saved)
        }
        _ => "n/a".to_string(),
    }
}

fn fmt_seconds(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:+.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn fmt_percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:+.1}%"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_signed_i128(value: i128) -> String {
    if value >= 0 {
        format!("+{value}")
    } else {
        value.to_string()
    }
}

fn copy_repo(source: &Path, destination: &Path) -> Result<()> {
    recreate_dir(destination)?;
    copy_dir_contents(source, destination, Path::new(""))
}

fn copy_dir_contents(source: &Path, destination: &Path, rel: &Path) -> Result<()> {
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        let child_rel = rel.join(&file_name);
        if should_skip(&child_rel) {
            continue;
        }

        let source_path = entry.path();
        let destination_path = destination.join(&child_rel);
        let metadata = fs::symlink_metadata(&source_path)?;

        if metadata.file_type().is_symlink() {
            continue;
        }

        if metadata.is_dir() {
            fs::create_dir_all(&destination_path)?;
            copy_dir_contents(&source_path, destination, &child_rel)?;
        } else if metadata.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn should_skip(rel: &Path) -> bool {
    if rel == Path::new("benchmarking").join("out")
        || rel == Path::new("benchmarking").join("target")
        || rel == Path::new(".ai").join("graphify-light")
    {
        return true;
    }

    rel.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if matches!(
                    name.to_str(),
                    Some(".git")
                        | Some(".codex")
                        | Some("target")
                        | Some("node_modules")
                        | Some("graphify-out")
                        | Some(".pytest_cache")
                        | Some(".mypy_cache")
                        | Some(".ruff_cache")
                        | Some(".venv")
                        | Some("venv")
                )
        )
    })
}

fn recreate_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(())
}

fn write_command_log(path: &Path, result: &CommandResult) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    writeln!(file, "status: {}", result.status)?;
    writeln!(file, "seconds: {:.3}", result.seconds)?;
    writeln!(file, "\n--- stdout ---\n{}", result.stdout)?;
    writeln!(file, "\n--- stderr ---\n{}", result.stderr)?;
    Ok(())
}

fn combine_streams(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (true, true) => String::new(),
    }
}

fn running_in_container() -> bool {
    Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || fs::read_to_string("/proc/1/cgroup")
            .map(|content| {
                content.contains("docker")
                    || content.contains("kubepods")
                    || content.contains("containerd")
            })
            .unwrap_or(false)
}

fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}
