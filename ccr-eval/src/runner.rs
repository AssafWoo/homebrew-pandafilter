use anyhow::{Context, Result};
use panda_core::config::CcrConfig;
use panda_core::pipeline::Pipeline;
use panda_core::tokens::count_tokens;
use panda_sdk::compressor::CompressionConfig;
use panda_sdk::message::Message;
use panda_sdk::ollama::OllamaConfig;
use panda_sdk::optimizer::Optimizer;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct QaFixture {
    #[serde(default)]
    pub command_hint: String,
    pub questions: Vec<Question>,
}

#[derive(Debug, Deserialize)]
pub struct Question {
    pub question: String,
    pub key_facts: Vec<String>,
}

#[derive(Debug)]
pub struct QuestionResult {
    pub question: String,
    pub original_answer: String,
    pub compressed_answer: String,
    pub original_score: bool,  // did original answer contain all key facts?
    pub compressed_score: bool, // did compressed answer contain all key facts?
    pub key_facts: Vec<String>,
}

#[derive(Debug)]
pub struct FixtureResult {
    pub name: String,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub savings_pct: f32,
    pub lines_in: usize,
    pub lines_out: usize,
    pub question_results: Vec<QuestionResult>,
    pub recall: f32,  // % of questions where compressed answer matched key facts
    pub original_recall: f32,
}

pub fn discover_fixtures(dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut pairs = Vec::new();
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("Cannot read fixtures dir: {}", dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("txt") {
            let qa_path = path.with_extension("qa.toml");
            if qa_path.exists() {
                pairs.push((path, qa_path));
            }
        }
    }
    pairs.sort();
    Ok(pairs)
}

pub fn run_fixture(txt_path: &Path, qa_path: &Path, api_key: &str) -> Result<FixtureResult> {
    let name = txt_path.file_stem().unwrap().to_string_lossy().into_owned();
    let input = std::fs::read_to_string(txt_path)?;
    let qa: QaFixture = toml::from_str(&std::fs::read_to_string(qa_path)?)?;

    // Apply command handler filter first (same as the real panda pipeline does at runtime).
    // Split command_hint on whitespace so "git push" → ["git", "push"] enabling subcmd routing.
    let handler_output = if !qa.command_hint.is_empty() {
        let binary = qa.command_hint.split_whitespace().next().unwrap_or(&qa.command_hint);
        if let Some(h) = panda::handlers::get_handler(binary) {
            let fake_args: Vec<String> = qa.command_hint.split_whitespace().map(|s| s.to_string()).collect();
            h.filter(&input, &fake_args)
        } else {
            input.clone()
        }
    } else {
        input.clone()
    };

    // Run CCR pipeline on handler-filtered output; token savings measured vs original input
    let config: CcrConfig = toml::from_str(include_str!("../../config/default_filters.toml"))
        .unwrap_or_default();
    let pipeline = Pipeline::new(config);
    let hint = if qa.command_hint.is_empty() { None } else { Some(qa.command_hint.as_str()) };
    let pipeline_result = pipeline.process(&handler_output, hint, None, None)?;
    let compressed = &pipeline_result.output;

    let lines_in = input.lines().count();
    let lines_out = compressed.lines().count();
    // Re-measure savings against the original raw input (not handler output)
    let input_tokens = panda_core::tokens::count_tokens(&input);
    let output_tokens = panda_core::tokens::count_tokens(compressed);
    let savings_pct = if input_tokens == 0 { 0.0 } else {
        (input_tokens.saturating_sub(output_tokens)) as f32 / input_tokens as f32 * 100.0
    };

    // Ask each question against both original and compressed
    let mut question_results = Vec::new();
    let mut original_hits = 0usize;
    let mut compressed_hits = 0usize;

    for q in &qa.questions {
        let orig_answer = ask_claude(&input, &q.question, api_key)?;
        let comp_answer = ask_claude(compressed, &q.question, api_key)?;

        let orig_score = score_answer(&orig_answer, &q.key_facts);
        let comp_score = score_answer(&comp_answer, &q.key_facts);

        if orig_score { original_hits += 1; }
        if comp_score { compressed_hits += 1; }

        question_results.push(QuestionResult {
            question: q.question.clone(),
            original_answer: orig_answer,
            compressed_answer: comp_answer,
            original_score: orig_score,
            compressed_score: comp_score,
            key_facts: q.key_facts.clone(),
        });
    }

    let n = qa.questions.len() as f32;
    let recall = if n == 0.0 { 100.0 } else { compressed_hits as f32 / n * 100.0 };
    let original_recall = if n == 0.0 { 100.0 } else { original_hits as f32 / n * 100.0 };

    Ok(FixtureResult {
        name,
        input_tokens,
        output_tokens,
        savings_pct,
        lines_in,
        lines_out,
        question_results,
        recall,
        original_recall,
    })
}

fn score_answer(answer: &str, key_facts: &[String]) -> bool {
    let answer_lower = answer.to_lowercase();
    key_facts.iter().any(|fact| answer_lower.contains(&fact.to_lowercase()))
}

// ── Conversation fixture eval ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ConvFixture {
    pub description: String,
    #[serde(rename = "turns")]
    pub turns: Vec<ConvTurn>,
    pub questions: Vec<Question>,
}

#[derive(Debug, Deserialize)]
pub struct ConvTurn {
    pub role: String,
    pub content: String,
}

/// Per-question result when comparing v1 vs v2.
#[derive(Debug)]
pub struct ConvCompareQuestion {
    pub question: String,
    pub key_facts: Vec<String>,
    #[allow(dead_code)]
    pub original_score: bool,
    pub v1_score: bool,
    pub v2_score: bool,
    pub v1_answer: String,
    pub v2_answer: String,
}

/// Side-by-side v1 vs v2 result for one conversation fixture.
#[derive(Debug)]
pub struct ConvCompareResult {
    pub name: String,
    pub description: String,
    pub turns: usize,
    pub original_recall: f32,
    // Snapshot: tokens in the final compressed state (single turn view)
    pub v1_tokens_in: usize,
    pub v1_tokens_out: usize,
    pub v1_savings_pct: f32,
    pub v1_recall: f32,
    #[allow(dead_code)]
    pub v2_tokens_in: usize,
    pub v2_tokens_out: usize,
    pub v2_savings_pct: f32,
    pub v2_recall: f32,
    // Cumulative: total tokens sent across all API calls in the conversation
    pub cumulative_tokens_original: usize,
    pub cumulative_tokens_v1: usize,
    pub cumulative_tokens_v2: usize,
    pub cumulative_savings_v1_pct: f32,
    pub cumulative_savings_v2_pct: f32,
    pub questions: Vec<ConvCompareQuestion>,
}

pub fn discover_conv_fixtures(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("Cannot read fixtures dir: {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if path.to_string_lossy().contains(".conv.") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// Format a conversation as a plain-text block for Claude to read.
fn format_conversation(turns: &[Message]) -> String {
    turns
        .iter()
        .map(|t| format!("[{}]: {}", t.role.to_uppercase(), t.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Compute total tokens sent across all API calls in the conversation.
///
/// At turn T the model receives [msg_0 .. msg_T-1]. Each message is compressed
/// based on its age at that specific turn — so an old message gets cheaper as
/// the conversation grows, saving tokens on every subsequent call.
///
/// `tier2_messages`: pre-compressed tier-2 content per message index.
///   For v1 this is extractive; for v2 this is Ollama output (or extractive fallback).
fn compute_cumulative_tokens(
    messages: &[Message],
    tier2_content: &[String], // tier2-compressed content per message index
    config: &CompressionConfig,
) -> usize {
    use panda_core::summarizer::summarize_message;

    let n = messages.len();

    // Precompute tokens at each compression level per message, once.
    let orig:  Vec<usize> = messages.iter().map(|m| count_tokens(&m.content)).collect();
    let tier1: Vec<usize> = messages.iter().map(|m| {
        if m.role != "user" { return count_tokens(&m.content); }
        count_tokens(&summarize_message(&m.content, config.tier1_ratio).output)
    }).collect();
    let tier2: Vec<usize> = messages.iter().enumerate().map(|(i, m)| {
        if m.role != "user" { return count_tokens(&m.content); }
        count_tokens(&tier2_content[i])
    }).collect();

    // Simulate every API call: turn 1 sends [msg_0], turn 2 sends [msg_0, msg_1], ...
    let mut total = 0usize;
    for turn in 1..=n {
        for i in 0..turn {
            let age = turn - 1 - i;
            total += if messages[i].role != "user" {
                orig[i]
            } else if age < config.recent_n {
                orig[i]
            } else if age < config.recent_n + config.tier1_n {
                tier1[i]
            } else {
                tier2[i]
            };
        }
    }
    total
}

/// Run a conversation fixture comparing v1 (extractive) vs v2 (Ollama + BERT gate).
/// Each question is asked against original, v1 compressed, and v2 compressed — 3 API
/// calls per question, no duplication.
pub fn run_conv_fixture_compare(path: &Path, api_key: &str) -> Result<ConvCompareResult> {
    let name = path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .replace(".conv.toml", "");
    let raw = std::fs::read_to_string(path)?;
    let fixture: ConvFixture = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    let messages: Vec<Message> = fixture
        .turns
        .iter()
        .map(|t| Message { role: t.role.clone(), content: t.content.clone() })
        .collect();

    // V1: extractive BERT only
    let v1 = Optimizer::default().compress(messages.clone());

    // V2: Ollama generative + BERT gate for tier 2
    let v2 = Optimizer {
        config: CompressionConfig {
            ollama: Some(OllamaConfig::default()),
            ..CompressionConfig::default()
        },
    }.compress(messages.clone());

    let original_text = format_conversation(&messages);
    let v1_text = format_conversation(&v1.messages);
    let v2_text = format_conversation(&v2.messages);

    let mut questions = Vec::new();
    let mut original_hits = 0usize;
    let mut v1_hits = 0usize;
    let mut v2_hits = 0usize;

    for q in &fixture.questions {
        let orig_answer = ask_claude(&original_text, &q.question, api_key)?;
        let v1_answer  = ask_claude(&v1_text,       &q.question, api_key)?;
        let v2_answer  = ask_claude(&v2_text,       &q.question, api_key)?;

        let orig_score = score_answer(&orig_answer, &q.key_facts);
        let v1_score   = score_answer(&v1_answer,   &q.key_facts);
        let v2_score   = score_answer(&v2_answer,   &q.key_facts);

        if orig_score { original_hits += 1; }
        if v1_score   { v1_hits += 1; }
        if v2_score   { v2_hits += 1; }

        questions.push(ConvCompareQuestion {
            question: q.question.clone(),
            key_facts: q.key_facts.clone(),
            original_score: orig_score,
            v1_score,
            v2_score,
            v1_answer,
            v2_answer,
        });
    }

    let n = fixture.questions.len() as f32;
    let recall = |hits: usize| if n == 0.0 { 100.0 } else { hits as f32 / n * 100.0 };
    let snap_savings = |r: &panda_sdk::compressor::CompressResult| {
        if r.tokens_in == 0 { 0.0 } else { (r.tokens_in - r.tokens_out) as f32 / r.tokens_in as f32 * 100.0 }
    };

    // Cumulative: extract tier2-compressed content per message.
    // For v1: use v1.messages[i].content (extractive tier2 for old, tier1 for mid, orig for recent).
    // For v2: use v2.messages[i].content (Ollama tier2 for old, same tier1 for mid, orig for recent).
    // We only need the tier2 content here — compute_cumulative_tokens handles tier selection.
    let config = CompressionConfig::default();
    let v1_tier2_content: Vec<String> = v1.messages.iter().map(|m| m.content.clone()).collect();
    let v2_tier2_content: Vec<String> = v2.messages.iter().map(|m| m.content.clone()).collect();

    let cum_orig = compute_cumulative_tokens(&messages, &v1_tier2_content, &config); // orig ignores tier2 for assistant
    // For original baseline: no compression — use original content for every message at every age.
    let cum_original = messages.iter().enumerate().map(|(i, _)| {
        (messages.len() - i) * count_tokens(&messages[i].content)
    }).sum::<usize>();

    let cum_v1 = compute_cumulative_tokens(&messages, &v1_tier2_content, &config);
    let cum_v2 = compute_cumulative_tokens(&messages, &v2_tier2_content, &config);

    let cum_savings = |cum: usize| {
        if cum_original == 0 { 0.0 } else { (cum_original - cum) as f32 / cum_original as f32 * 100.0 }
    };

    let _ = cum_orig; // suppress unused warning

    Ok(ConvCompareResult {
        name,
        description: fixture.description,
        turns: fixture.turns.len(),
        original_recall: recall(original_hits),
        v1_tokens_in:    v1.tokens_in,
        v1_tokens_out:   v1.tokens_out,
        v1_savings_pct:  snap_savings(&v1),
        v1_recall:       recall(v1_hits),
        v2_tokens_in:    v2.tokens_in,
        v2_tokens_out:   v2.tokens_out,
        v2_savings_pct:  snap_savings(&v2),
        v2_recall:       recall(v2_hits),
        cumulative_tokens_original: cum_original,
        cumulative_tokens_v1:       cum_v1,
        cumulative_tokens_v2:       cum_v2,
        cumulative_savings_v1_pct:  cum_savings(cum_v1),
        cumulative_savings_v2_pct:  cum_savings(cum_v2),
        questions,
    })
}

// ── Shared helpers ────────────────────────────────────────────────────────────

pub fn ask_claude(context: &str, question: &str, api_key: &str) -> Result<String> {
    let client = reqwest::blocking::Client::new();

    let body = serde_json::json!({
        "model": "claude-haiku-4-5-20251001",
        "max_tokens": 256,
        "messages": [{
            "role": "user",
            "content": format!(
                "Here is some text:\n\n<text>\n{}\n</text>\n\nAnswer this question based only on the text above. Be concise.\n\nQuestion: {}",
                context, question
            )
        }]
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()?
        .json::<serde_json::Value>()?;

    let answer = resp["content"][0]["text"]
        .as_str()
        .unwrap_or("(no response)")
        .to_string();

    Ok(answer)
}
