/// cl100k_base token counting.
///
/// Uses `bpe-openai` (GitHub rust-gems) instead of `tiktoken-rs`: both
/// implement the exact same cl100k_base BPE, but tiktoken-rs rebuilds the
/// 100k-entry merge table on first use (~45ms per process in release,
/// ~280ms in debug), which dominated panda's per-invocation latency.
/// bpe-openai ships precomputed tables and initializes in microseconds.
///
/// Counts are identical to tiktoken's `encode_ordinary` for all text. The
/// only divergence is input containing literal special-token markers such
/// as `<|endoftext|>` (previously counted as 1 token, now tokenized as
/// plain text) — irrelevant for analytics over command output.
pub fn count_tokens(text: &str) -> usize {
    bpe_openai::cl100k_base().count(text)
}

/// Force one-time tokenizer initialization (~20ms release). Spawn this on a
/// background thread at process entry so table construction overlaps stdin
/// reading and filtering instead of blocking the first `count_tokens` call.
pub fn warm() {
    let _ = bpe_openai::cl100k_base();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_zero_tokens() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn known_string_token_count() {
        // "hello world" is typically 2 tokens in cl100k_base
        let count = count_tokens("hello world");
        assert!(count > 0);
        assert!(count <= 5);
    }

    #[test]
    fn count_increases_with_longer_input() {
        let short = count_tokens("hello");
        let long = count_tokens("hello world this is a longer sentence with many more words");
        assert!(long > short);
    }

    #[test]
    fn unicode_text_counted() {
        let count = count_tokens("こんにちは世界");
        assert!(count > 0);
    }

    /// Parity with the previous tiktoken-rs implementation: bpe-openai must
    /// produce identical cl100k counts on representative command output.
    #[test]
    fn parity_with_tiktoken() {
        let samples: &[&str] = &[
            "hello world",
            "fn main() { println!(\"hi\"); }",
            "error[E0308]: mismatched types\n --> src/main.rs:4:5",
            " src/lib.rs                    |  42 ++++++----\n 23 files changed, 903 insertions(+), 668 deletions(-)",
            "こんにちは世界 — naïve café",
            "PASS src/__tests__/index.test.js (1.234 s)\nTests: 12 passed, 12 total",
            "{\"ok\": true, \"items\": [1, 2, 3], \"nested\": {\"a\": null}}",
            "    at Object.<anonymous> (/app/node_modules/express/lib/router/index.js:284:7)",
        ];
        let tk = tiktoken_rs::cl100k_base().unwrap();
        for s in samples {
            assert_eq!(
                count_tokens(s),
                tk.encode_ordinary(s).len(),
                "count mismatch for sample: {s:?}"
            );
        }
    }
}
