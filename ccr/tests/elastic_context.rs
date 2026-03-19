/// Integration tests for EC — Elastic Context.
///
/// Verifies that context_pressure() ramps correctly, with_pressure() adjusts the
/// pipeline config, and high-pressure sessions fire BERT sooner.
use ccr::session::SessionState;
use ccr_core::config::CcrConfig;

// ── context_pressure() ────────────────────────────────────────────────────────

#[test]
fn pressure_zero_for_fresh_session() {
    let s = SessionState::default();
    assert_eq!(s.context_pressure(), 0.0);
}

#[test]
fn pressure_zero_below_start_threshold() {
    let mut s = SessionState::default();
    s.total_tokens = 20_000; // below 25k start
    assert_eq!(s.context_pressure(), 0.0);
}

#[test]
fn pressure_ramps_linearly_at_midpoint() {
    let mut s = SessionState::default();
    // midpoint: (52.5k - 25k) / (80k - 25k) = 27.5k / 55k = 0.5
    s.total_tokens = 52_500;
    let p = s.context_pressure();
    assert!(
        (p - 0.5).abs() < 0.01,
        "expected ~0.5 at midpoint, got {}",
        p
    );
}

#[test]
fn pressure_caps_at_one_past_max() {
    let mut s = SessionState::default();
    s.total_tokens = 200_000;
    assert_eq!(s.context_pressure(), 1.0);
}

#[test]
fn pressure_at_exact_start_is_zero() {
    let mut s = SessionState::default();
    s.total_tokens = 25_000;
    assert_eq!(s.context_pressure(), 0.0);
}

#[test]
fn pressure_at_exact_max_is_one() {
    let mut s = SessionState::default();
    s.total_tokens = 80_000;
    assert_eq!(s.context_pressure(), 1.0);
}

// ── CcrConfig::with_pressure() ────────────────────────────────────────────────

#[test]
fn with_pressure_zero_is_identity() {
    let config = CcrConfig::default();
    let original_threshold = config.global.summarize_threshold_lines;
    let original_head = config.global.head_lines;
    let original_tail = config.global.tail_lines;
    let adjusted = config.with_pressure(0.0);
    assert_eq!(adjusted.global.summarize_threshold_lines, original_threshold);
    assert_eq!(adjusted.global.head_lines, original_head);
    assert_eq!(adjusted.global.tail_lines, original_tail);
}

#[test]
fn with_pressure_one_tightens_threshold() {
    let config = CcrConfig::default();
    let original = config.global.summarize_threshold_lines; // 200
    let adjusted = config.with_pressure(1.0);
    assert!(
        adjusted.global.summarize_threshold_lines < original / 2,
        "threshold should be less than half at p=1.0, got {}",
        adjusted.global.summarize_threshold_lines
    );
    assert!(
        adjusted.global.summarize_threshold_lines >= 30,
        "threshold must not go below minimum of 30"
    );
}

#[test]
fn with_pressure_one_tightens_budget() {
    let config = CcrConfig::default();
    let original_head = config.global.head_lines;
    let original_tail = config.global.tail_lines;
    let adjusted = config.with_pressure(1.0);
    assert!(adjusted.global.head_lines < original_head);
    assert!(adjusted.global.tail_lines < original_tail);
    assert!(adjusted.global.head_lines >= 4);
    assert!(adjusted.global.tail_lines >= 4);
}

#[test]
fn with_pressure_half_is_between_zero_and_one() {
    let base = CcrConfig::default().global.summarize_threshold_lines;
    let at_zero = CcrConfig::default().with_pressure(0.0).global.summarize_threshold_lines;
    let at_half = CcrConfig::default().with_pressure(0.5).global.summarize_threshold_lines;
    let at_one  = CcrConfig::default().with_pressure(1.0).global.summarize_threshold_lines;
    assert_eq!(at_zero, base);
    assert!(at_half < at_zero && at_half > at_one);
}

#[test]
fn with_pressure_respects_minimum_threshold() {
    // Even at p=1.0 with a very small configured threshold, minimum is 30.
    let mut config = CcrConfig::default();
    config.global.summarize_threshold_lines = 10; // tiny
    let adjusted = config.with_pressure(1.0);
    assert_eq!(adjusted.global.summarize_threshold_lines, 30);
}

#[test]
fn with_pressure_respects_minimum_budget() {
    let mut config = CcrConfig::default();
    config.global.head_lines = 5;
    config.global.tail_lines = 5;
    let adjusted = config.with_pressure(1.0);
    assert_eq!(adjusted.global.head_lines, 4);
    assert_eq!(adjusted.global.tail_lines, 4);
}

// ── pipeline integration ──────────────────────────────────────────────────────

#[test]
fn pipeline_fires_bert_sooner_under_high_pressure() {
    use ccr_core::pipeline::Pipeline;

    // 60 lines — below normal threshold (200) but above critical threshold (~50)
    let input: String = (0..60)
        .map(|i| format!("log line number {} with some extra words to pad it", i))
        .collect::<Vec<_>>()
        .join("\n");

    // Under no pressure: 60 lines < 200 threshold → no BERT → all lines kept
    let config_normal = CcrConfig::default();
    let result_normal = Pipeline::new(config_normal)
        .process(&input, None, None, None)
        .unwrap();
    assert!(
        result_normal.output.lines().count() >= 50,
        "no pressure should not summarize 60 lines, got {}",
        result_normal.output.lines().count()
    );

    // Under max pressure: threshold shrinks to ~50 → BERT fires → fewer lines
    let config_pressure = CcrConfig::default().with_pressure(1.0);
    let result_pressure = Pipeline::new(config_pressure)
        .process(&input, None, None, None)
        .unwrap();
    assert!(
        result_pressure.output.lines().count() < result_normal.output.lines().count(),
        "high pressure should produce fewer lines than no pressure"
    );
}

// ── regression: existing compression_factor() unchanged ──────────────────────

#[test]
fn compression_factor_unchanged_by_pressure_feature() {
    let mut s = SessionState::default();
    assert_eq!(s.compression_factor(), 1.0);

    s.total_tokens = 100_000;
    let cf = s.compression_factor();
    assert!(cf < 1.0 && cf >= 0.5, "compression_factor should be in [0.5, 1.0], got {}", cf);
}
