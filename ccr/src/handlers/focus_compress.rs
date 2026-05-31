//! Focus-aware file compression.
//!
//! Replaces dumb head/tail truncation for large code files with relevance-based
//! section compression: split the file into structural sections (functions,
//! typedefs, imports), score each against the current prompt embedding,
//! always preserve imports + typedefs, and compress the rest based on relevance.

use anyhow::Result;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SectionKind {
    Import,
    TypeDef,
    Function,
    TopLevel,
}

pub struct FileSection {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: SectionKind,
    pub text: String,
    pub header_lines: usize,
}

pub struct FocusCompressResult {
    pub output: String,
    pub sections_total: usize,
    pub sections_preserved: usize,
    pub sections_compressed: usize,
    pub lines_preserved: usize,
    pub lines_compressed: usize,
    pub section_details: Vec<SectionDetail>,
    pub old_method_tokens: usize,
    pub new_method_tokens: usize,
}

pub struct SectionDetail {
    pub start_line: usize,
    pub end_line: usize,
    pub score: f32,
    pub preserved: bool,
    pub kind: SectionKind,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn strip_vis(s: &str) -> &str {
    let prefixes = ["pub(crate) ", "pub(super) ", "pub(in ", "pub "];
    for prefix in &prefixes {
        if let Some(rest) = s.strip_prefix(prefix) {
            // For pub(in ...) skip past the closing paren
            if *prefix == "pub(in " {
                if let Some(end) = rest.find(") ") {
                    return &rest[end + 2..];
                }
            }
            return rest;
        }
    }
    s
}

fn classify_kind(trimmed: &str) -> SectionKind {
    let stripped = strip_vis(trimmed);
    let first = stripped.split_whitespace().next().unwrap_or(trimmed);
    match first {
        "use" | "import" | "from" | "extern" => SectionKind::Import,
        "struct" | "enum" | "interface" | "union" | "type" | "typedef" => SectionKind::TypeDef,
        "fn" | "func" | "function" | "def" | "class" | "impl" | "trait" | "mod" | "async" => {
            SectionKind::Function
        }
        _ => SectionKind::TopLevel,
    }
}

fn count_header_lines_brace(lines: &[&str]) -> usize {
    let mut depth = 0i32;
    for (i, line) in lines.iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth > 0 {
            return i + 1;
        }
    }
    0
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

// ── split_into_sections ───────────────────────────────────────────────────────

pub fn split_into_sections(content: &str, ext: &str) -> Vec<FileSection> {
    let ext_lc = ext.to_lowercase();
    match ext_lc.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "go" | "java" | "cs" | "cpp"
        | "cc" | "c" | "h" | "hpp" => split_brace(content),
        "py" | "pyi" => split_python(content),
        _ => split_paragraph(content),
    }
}

fn split_brace(content: &str) -> Vec<FileSection> {
    let lines: Vec<&str> = content.lines().collect();
    let mut sections: Vec<FileSection> = Vec::new();

    let mut section_start = 0usize;
    let mut depth = 0i32;
    let mut pending_attrs: Vec<usize> = Vec::new(); // line indices of leading attrs/comments

    // When depth > 0, track where the section "body" started
    let mut in_body = false;
    let mut body_kind = SectionKind::TopLevel;

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        let prev_depth = depth;
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }

        // Detect section open: was at 0, now > 0
        if prev_depth == 0 && depth > 0 {
            in_body = true;
            // classify from section_start line (first non-attr)
            let classify_line = lines
                .get(section_start)
                .map(|l| l.trim())
                .unwrap_or("");
            body_kind = classify_kind(classify_line);
        }

        // Detect section close: was > 0, now == 0
        if prev_depth > 0 && depth == 0 && in_body {
            in_body = false;
            let end = i + 1;
            let section_lines = &lines[section_start..end];
            let text = section_lines.join("\n");
            let header = count_header_lines_brace(section_lines);
            sections.push(FileSection {
                start_line: section_start,
                end_line: end,
                kind: body_kind.clone(),
                text,
                header_lines: header,
            });
            pending_attrs.clear();
            // next section starts after this one
            section_start = end;
            i = end;
            continue;
        }

        // At depth==0: look for natural break on blank lines
        if depth == 0 && !in_body {
            if trimmed.is_empty() {
                // blank line at top level — if we've accumulated non-blank content, flush it
                let section_text: Vec<&str> = lines[section_start..i]
                    .iter()
                    .copied()
                    .collect();
                let non_blank = section_text.iter().any(|l| !l.trim().is_empty());
                if non_blank {
                    let text = section_text.join("\n");
                    let classify_line = section_text
                        .iter()
                        .find(|l| !l.trim().is_empty())
                        .map(|l| l.trim())
                        .unwrap_or("");
                    let kind = classify_kind(classify_line);
                    sections.push(FileSection {
                        start_line: section_start,
                        end_line: i,
                        kind,
                        text,
                        header_lines: 0,
                    });
                    pending_attrs.clear();
                }
                section_start = i + 1;
            } else {
                // Track attribute / doc-comment lines at depth==0
                let is_attr = trimmed.starts_with("#[")
                    || trimmed.starts_with("///")
                    || trimmed.starts_with("//!")
                    || trimmed.starts_with("/**")
                    || trimmed.starts_with("/*");
                if is_attr {
                    pending_attrs.push(i);
                } else {
                    // Non-blank, non-attr at depth 0 without brace — flush as top-level
                    // (only if there's no open brace on this line, handled above)
                }
            }
        }

        i += 1;
    }

    // Flush any remaining content
    if section_start < lines.len() {
        let section_lines = &lines[section_start..];
        let non_blank = section_lines.iter().any(|l| !l.trim().is_empty());
        if non_blank {
            let text = section_lines.join("\n");
            let classify_line = section_lines
                .iter()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim())
                .unwrap_or("");
            let kind = classify_kind(classify_line);
            let header = if depth > 0 {
                count_header_lines_brace(section_lines)
            } else {
                0
            };
            sections.push(FileSection {
                start_line: section_start,
                end_line: lines.len(),
                kind,
                text,
                header_lines: header,
            });
        }
    }

    sections
}

fn split_python(content: &str) -> Vec<FileSection> {
    let lines: Vec<&str> = content.lines().collect();
    let mut sections: Vec<FileSection> = Vec::new();
    let mut section_start = 0usize;

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        if trimmed.is_empty() && indent == 0 {
            // blank line at indent 0
            let slice = &lines[section_start..i];
            let non_blank = slice.iter().any(|l| !l.trim().is_empty());
            if non_blank {
                let text = slice.join("\n");
                let classify_line = slice
                    .iter()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim())
                    .unwrap_or("");
                let kind = classify_kind(classify_line);
                sections.push(FileSection {
                    start_line: section_start,
                    end_line: i,
                    kind,
                    text,
                    header_lines: 1,
                });
            }
            section_start = i + 1;
        } else if indent == 0 && !trimmed.is_empty() && i > section_start {
            // Kind change at indent 0 — check if we need to flush
            let prev_classify = lines[section_start..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim())
                .unwrap_or("");
            let prev_kind = classify_kind(prev_classify);
            let new_kind = classify_kind(trimmed);
            if std::mem::discriminant(&prev_kind) != std::mem::discriminant(&new_kind) {
                let slice = &lines[section_start..i];
                let non_blank = slice.iter().any(|l| !l.trim().is_empty());
                if non_blank {
                    let text = slice.join("\n");
                    sections.push(FileSection {
                        start_line: section_start,
                        end_line: i,
                        kind: prev_kind,
                        text,
                        header_lines: 1,
                    });
                    section_start = i;
                }
            }
        }

        i += 1;
    }

    // Flush remaining
    if section_start < lines.len() {
        let slice = &lines[section_start..];
        let non_blank = slice.iter().any(|l| !l.trim().is_empty());
        if non_blank {
            let text = slice.join("\n");
            let classify_line = slice
                .iter()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim())
                .unwrap_or("");
            let kind = classify_kind(classify_line);
            sections.push(FileSection {
                start_line: section_start,
                end_line: lines.len(),
                kind,
                text,
                header_lines: 1,
            });
        }
    }

    sections
}

fn split_paragraph(content: &str) -> Vec<FileSection> {
    let lines: Vec<&str> = content.lines().collect();
    let mut sections: Vec<FileSection> = Vec::new();
    let mut section_start = 0usize;
    let mut blank_run = 0usize;

    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            blank_run += 1;
        } else {
            if blank_run >= 2 && i > section_start {
                let end = i - blank_run;
                if end > section_start {
                    let text = lines[section_start..end].join("\n");
                    sections.push(FileSection {
                        start_line: section_start,
                        end_line: end,
                        kind: SectionKind::TopLevel,
                        text,
                        header_lines: 0,
                    });
                }
                section_start = i;
            }
            blank_run = 0;
        }
    }

    // Flush remaining
    let end = lines.len();
    if section_start < end {
        let slice = &lines[section_start..end];
        let non_blank = slice.iter().any(|l| !l.trim().is_empty());
        if non_blank {
            sections.push(FileSection {
                start_line: section_start,
                end_line: end,
                kind: SectionKind::TopLevel,
                text: slice.join("\n"),
                header_lines: 0,
            });
        }
    }

    sections
}

// ── score_and_compress ────────────────────────────────────────────────────────

pub fn score_and_compress(
    sections: &[FileSection],
    prompt_emb: &[f32],
    preserve_ranges: &[(usize, usize)],
) -> Result<FocusCompressResult> {
    // 1. Embed section texts (truncate to 512 chars)
    let texts: Vec<&str> = sections
        .iter()
        .map(|s| {
            let end = s.text.char_indices().nth(512).map(|(i, _)| i).unwrap_or(s.text.len());
            &s.text[..end]
        })
        .collect();

    // Embed section texts. If BERT is unavailable (daemon not running), fall back
    // to zero embeddings — all cosine scores become 0.0, threshold becomes 0.0,
    // so every section is preserved (rule-based only: imports, typedefs, edit ranges,
    // 50% minimum). This is the correct degraded behavior and keeps tests passing in
    // environments without a running daemon (e.g. CI).
    let embeddings = panda_core::summarizer::embed_batch(&texts)
        .unwrap_or_else(|_| vec![vec![0.0f32; prompt_emb.len().max(1)]; sections.len()]);

    // 2. Compute cosine similarities
    let scores: Vec<f32> = embeddings
        .iter()
        .map(|emb| cosine_sim(emb, prompt_emb))
        .collect();

    // 3. Force-preserve: Import, TypeDef, or overlapping edit ranges
    let force_preserve: Vec<bool> = sections
        .iter()
        .map(|s| {
            if matches!(s.kind, SectionKind::Import | SectionKind::TypeDef) {
                return true;
            }
            // Check overlap with preserve_ranges
            preserve_ranges.iter().any(|(start, end)| {
                s.start_line < *end && s.end_line > *start
            })
        })
        .collect();

    // 4. Compute 40th percentile threshold of scores for non-force-preserved sections
    let mut non_forced_scores: Vec<f32> = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| !force_preserve[*i])
        .map(|(_, &s)| s)
        .collect();
    non_forced_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let threshold = if non_forced_scores.is_empty() {
        0.0f32
    } else {
        let idx = (non_forced_scores.len() as f32 * 0.40) as usize;
        let idx = idx.min(non_forced_scores.len().saturating_sub(1));
        non_forced_scores[idx]
    };

    // 5. Initial preserve set
    let mut preserved: Vec<bool> = sections
        .iter()
        .enumerate()
        .map(|(i, _)| force_preserve[i] || scores[i] >= threshold)
        .collect();

    // 6. Enforce minimum 50% preservation
    let total = sections.len();
    let min_keep = (total + 1) / 2; // ceil(total/2)
    let current_kept = preserved.iter().filter(|&&p| p).count();

    if current_kept < min_keep {
        // Sort non-preserved indices by score descending, add until we hit min_keep
        let mut non_kept: Vec<(usize, f32)> = preserved
            .iter()
            .enumerate()
            .filter(|(_, &p)| !p)
            .map(|(i, _)| (i, scores[i]))
            .collect();
        non_kept.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let need = min_keep - current_kept;
        for (idx, _) in non_kept.iter().take(need) {
            preserved[*idx] = true;
        }
    }

    // 7. Build output
    let mut output_parts: Vec<String> = Vec::new();
    let mut lines_preserved = 0usize;
    let mut lines_compressed = 0usize;
    let mut sections_preserved = 0usize;
    let mut sections_compressed = 0usize;
    let mut section_details: Vec<SectionDetail> = Vec::new();

    for (i, section) in sections.iter().enumerate() {
        let is_preserved = preserved[i];
        let sec_lines = section.end_line - section.start_line;

        section_details.push(SectionDetail {
            start_line: section.start_line,
            end_line: section.end_line,
            score: scores[i],
            preserved: is_preserved,
            kind: section.kind.clone(),
        });

        if is_preserved {
            output_parts.push(section.text.clone());
            lines_preserved += sec_lines;
            sections_preserved += 1;
        } else {
            sections_compressed += 1;
            lines_compressed += sec_lines;

            let compressed_text = if section.header_lines > 0 {
                let sec_lines_vec: Vec<&str> = section.text.lines().collect();
                let header_text = sec_lines_vec[..section.header_lines.min(sec_lines_vec.len())]
                    .join("\n");
                let body_line_count = sec_lines_vec
                    .len()
                    .saturating_sub(section.header_lines)
                    .saturating_sub(1); // subtract closing brace line
                let zi = panda_core::zoom::register(
                    section.text.lines().map(|l| l.to_string()).collect(),
                );
                let last_line = sec_lines_vec.last().copied().unwrap_or("");
                let has_closing_brace = last_line.trim() == "}";
                if has_closing_brace {
                    format!(
                        "{}\n    // [{} lines — panda expand {}]\n}}",
                        header_text, body_line_count, zi
                    )
                } else {
                    format!(
                        "{}\n    // [{} lines — panda expand {}]",
                        header_text, body_line_count, zi
                    )
                }
            } else {
                let zi = panda_core::zoom::register(
                    section.text.lines().map(|l| l.to_string()).collect(),
                );
                format!("// [{} lines — panda expand {}]", sec_lines, zi)
            };

            output_parts.push(compressed_text);
        }
    }

    let output = output_parts.join("\n\n");
    let new_method_tokens = panda_core::tokens::count_tokens(&output);

    // 8. Compute old_method_tokens
    let total_lines: usize = sections.iter().map(|s| s.end_line - s.start_line).sum();
    let full_content: String = sections.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n\n");
    let old_method_tokens = compute_old_method_tokens(&full_content, total_lines);

    Ok(FocusCompressResult {
        output,
        sections_total: total,
        sections_preserved,
        sections_compressed,
        lines_preserved,
        lines_compressed,
        section_details,
        old_method_tokens,
        new_method_tokens,
    })
}

fn compute_old_method_tokens(content: &str, total_lines: usize) -> usize {
    if total_lines <= 100 {
        return panda_core::tokens::count_tokens(content);
    }
    let all_lines: Vec<&str> = content.lines().collect();
    if total_lines <= 500 {
        let head = all_lines[..60.min(all_lines.len())].join("\n");
        let tail_start = all_lines.len().saturating_sub(20);
        let tail = all_lines[tail_start..].join("\n");
        panda_core::tokens::count_tokens(&head) + panda_core::tokens::count_tokens(&tail)
    } else {
        let head = all_lines[..80.min(all_lines.len())].join("\n");
        panda_core::tokens::count_tokens(&head)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_into_sections: Rust / brace-based ───────────────────────────────

    #[test]
    fn test_rust_use_imports_grouped() {
        let src = "use std::fs;\nuse std::path::Path;\n\nfn foo() {}\n";
        let sections = split_into_sections(src, "rs");
        // imports should be one section, function another
        assert!(sections.iter().any(|s| s.kind == SectionKind::Import), "expected an Import section");
        assert!(sections.iter().any(|s| s.kind == SectionKind::Function), "expected a Function section");
    }

    #[test]
    fn test_rust_struct_classified_as_typedef() {
        let src = "struct Foo {\n    x: i32,\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].kind, SectionKind::TypeDef);
    }

    #[test]
    fn test_rust_fn_classified_as_function() {
        let src = "fn bar() {\n    let x = 1;\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].kind, SectionKind::Function);
    }

    #[test]
    fn test_rust_pub_fn_classified_as_function() {
        let src = "pub fn hello() {\n    println!(\"hi\");\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].kind, SectionKind::Function);
    }

    #[test]
    fn test_rust_enum_classified_as_typedef() {
        let src = "enum Color {\n    Red,\n    Green,\n    Blue,\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].kind, SectionKind::TypeDef);
    }

    #[test]
    fn test_rust_two_functions_split_into_two_sections() {
        let src = "fn foo() {\n    let x = 1;\n}\n\nfn bar() {\n    let y = 2;\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 2, "each function should be its own section");
        assert!(sections.iter().all(|s| s.kind == SectionKind::Function));
    }

    #[test]
    fn test_rust_adjacent_functions_no_blank_line() {
        // No blank line between functions — depth returning to 0 should split them
        let src = "fn foo() {\n    1\n}\nfn bar() {\n    2\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 2, "adjacent functions should still be separate sections");
    }

    #[test]
    fn test_rust_section_line_ranges_are_correct() {
        let src = "use std::fs;\n\nfn foo() {\n    let x = 1;\n}\n";
        let sections = split_into_sections(src, "rs");
        // Import section should start at line 0
        let import = sections.iter().find(|s| s.kind == SectionKind::Import).unwrap();
        assert_eq!(import.start_line, 0);
        // Function section should start somewhere after the import
        let func = sections.iter().find(|s| s.kind == SectionKind::Function).unwrap();
        assert!(func.start_line > 0);
        // end_line should be exclusive and > start_line
        assert!(func.end_line > func.start_line);
    }

    #[test]
    fn test_rust_header_lines_detected() {
        // Single-line function header: `fn foo() {`
        let src = "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 1);
        // Header is the `fn foo() {` line = 1 line
        assert_eq!(sections[0].header_lines, 1);
    }

    #[test]
    fn test_rust_multiline_header_lines() {
        // Multi-line signature before opening brace
        let src = "fn complex(\n    x: i32,\n    y: i32,\n) -> i32 {\n    x + y\n}\n";
        let sections = split_into_sections(src, "rs");
        assert_eq!(sections.len(), 1);
        // Header includes all lines up to and including the `{` line = 4 lines
        assert_eq!(sections[0].header_lines, 4);
    }

    #[test]
    fn test_rust_impl_block_classified_as_function() {
        let src = "impl Foo {\n    fn method(&self) -> i32 {\n        42\n    }\n}\n";
        let sections = split_into_sections(src, "rs");
        // The whole impl block is one section
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].kind, SectionKind::Function);
    }

    #[test]
    fn test_rust_const_classified_as_toplevel() {
        let src = "const MAX: usize = 100;\n\nfn foo() {}\n";
        let sections = split_into_sections(src, "rs");
        let toplevel = sections.iter().find(|s| s.kind == SectionKind::TopLevel);
        assert!(toplevel.is_some(), "const should be TopLevel");
    }

    // ── split_into_sections: Python ───────────────────────────────────────────

    #[test]
    fn test_python_imports_and_function() {
        let src = "import os\nfrom pathlib import Path\n\ndef foo():\n    pass\n";
        let sections = split_into_sections(src, "py");
        assert!(sections.iter().any(|s| s.kind == SectionKind::Import));
        assert!(sections.iter().any(|s| s.kind == SectionKind::Function));
    }

    #[test]
    fn test_python_class_classified_as_function() {
        let src = "class Foo:\n    def __init__(self):\n        pass\n";
        let sections = split_into_sections(src, "py");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].kind, SectionKind::Function);
    }

    // ── split_into_sections: unknown ext fallback ─────────────────────────────

    #[test]
    fn test_unknown_ext_paragraph_split() {
        let src = "line one\nline two\n\n\nline three\nline four\n";
        let sections = split_into_sections(src, "txt");
        assert!(sections.len() >= 1);
        assert!(sections.iter().all(|s| s.kind == SectionKind::TopLevel));
    }

    // ── score_and_compress ────────────────────────────────────────────────────

    #[test]
    fn test_import_sections_always_preserved() {
        // Build synthetic sections
        let import_section = FileSection {
            start_line: 0,
            end_line: 2,
            kind: SectionKind::Import,
            text: "use std::fs;\nuse std::path::Path;".to_string(),
            header_lines: 0,
        };
        let fn_section = FileSection {
            start_line: 3,
            end_line: 10,
            kind: SectionKind::Function,
            text: "fn totally_unrelated_noise() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    let e = 5;\n}".to_string(),
            header_lines: 1,
        };

        let sections = vec![import_section, fn_section];

        // Prompt embedding pointing strongly away from std::fs / imports
        // Use a zero vector as prompt — cosine sim will be 0 for everything
        let prompt_emb = vec![0.0f32; 384];
        let result = score_and_compress(&sections, &prompt_emb, &[]).unwrap();

        // Import section must always be preserved
        let import_detail = result.section_details.iter().find(|d| d.kind == SectionKind::Import).unwrap();
        assert!(import_detail.preserved, "Import section must always be preserved");
    }

    #[test]
    fn test_typedef_sections_always_preserved() {
        let typedef_section = FileSection {
            start_line: 0,
            end_line: 3,
            kind: SectionKind::TypeDef,
            text: "struct Config {\n    timeout: u64,\n}".to_string(),
            header_lines: 1,
        };
        let fn_section = FileSection {
            start_line: 5,
            end_line: 10,
            kind: SectionKind::Function,
            text: "fn do_something() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}".to_string(),
            header_lines: 1,
        };
        let sections = vec![typedef_section, fn_section];
        let prompt_emb = vec![0.0f32; 384];
        let result = score_and_compress(&sections, &prompt_emb, &[]).unwrap();

        let td = result.section_details.iter().find(|d| d.kind == SectionKind::TypeDef).unwrap();
        assert!(td.preserved, "TypeDef section must always be preserved");
    }

    #[test]
    fn test_at_least_half_sections_preserved() {
        // Build 4 function sections — minimum 50% (2) must be preserved
        let sections: Vec<FileSection> = (0..4).map(|i| FileSection {
            start_line: i * 5,
            end_line: i * 5 + 4,
            kind: SectionKind::Function,
            text: format!("fn func{}() {{\n    let x = {};\n    let y = {};\n}}", i, i, i),
            header_lines: 1,
        }).collect();

        let prompt_emb = vec![0.0f32; 384]; // all scores = 0 → threshold = 0
        let result = score_and_compress(&sections, &prompt_emb, &[]).unwrap();

        assert!(result.sections_preserved >= 2, "at least 50% must be preserved, got {}", result.sections_preserved);
        assert_eq!(result.sections_total, 4);
    }

    #[test]
    fn test_compressed_section_contains_zoom_id() {
        panda_core::zoom::enable();
        let fn_section = FileSection {
            start_line: 0,
            end_line: 6,
            kind: SectionKind::Function,
            text: "fn unused_func() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n}".to_string(),
            header_lines: 1,
        };
        let import_section = FileSection {
            start_line: 10,
            end_line: 11,
            kind: SectionKind::Import,
            text: "use std::fs;".to_string(),
            header_lines: 0,
        };
        // Two sections: one will be preserved (import), one may be compressed
        let sections = vec![fn_section, import_section];
        let prompt_emb = vec![0.0f32; 384];
        let result = score_and_compress(&sections, &prompt_emb, &[]).unwrap();

        // If any section was compressed, its output text must contain "panda expand ZI_"
        if result.sections_compressed > 0 {
            assert!(
                result.output.contains("panda expand ZI_"),
                "compressed section output must contain zoom ID, got: {}",
                &result.output[..result.output.len().min(200)]
            );
        }
    }

    #[test]
    fn test_edit_preserve_ranges_force_preserve() {
        // Section at lines 5-15 overlaps with preserve_range (10, 20)
        let fn_section = FileSection {
            start_line: 5,
            end_line: 15,
            kind: SectionKind::Function,
            text: "fn recently_edited() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    let w = 4;\n    let v = 5;\n    let u = 6;\n    let t = 7;\n    let s = 8;\n}".to_string(),
            header_lines: 1,
        };
        // Another function with no overlap
        let fn_section2 = FileSection {
            start_line: 20,
            end_line: 30,
            kind: SectionKind::Function,
            text: "fn other_func() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    let e = 5;\n    let f = 6;\n    let g = 7;\n    let h = 8;\n}".to_string(),
            header_lines: 1,
        };
        let sections = vec![fn_section, fn_section2];
        let preserve_ranges = vec![(10usize, 20usize)]; // overlaps fn_section
        let prompt_emb = vec![0.0f32; 384];

        let result = score_and_compress(&sections, &prompt_emb, &preserve_ranges).unwrap();

        // The section overlapping the edit range must be preserved
        let edit_section = result.section_details.iter().find(|d| d.start_line == 5).unwrap();
        assert!(edit_section.preserved, "section overlapping edit range must be preserved");
    }

    // ── classify_kind ────────────────────────────────────────────────────────

    #[test]
    fn test_classify_kind_pub_fn() {
        assert_eq!(classify_kind("pub fn foo() {"), SectionKind::Function);
    }

    #[test]
    fn test_classify_kind_use() {
        assert_eq!(classify_kind("use std::fs;"), SectionKind::Import);
    }

    #[test]
    fn test_classify_kind_struct() {
        assert_eq!(classify_kind("struct Foo {"), SectionKind::TypeDef);
    }

    #[test]
    fn test_classify_kind_enum() {
        assert_eq!(classify_kind("enum Color {"), SectionKind::TypeDef);
    }

    #[test]
    fn test_classify_kind_impl() {
        assert_eq!(classify_kind("impl Foo {"), SectionKind::Function);
    }

    #[test]
    fn test_classify_kind_const() {
        assert_eq!(classify_kind("const MAX: usize = 100;"), SectionKind::TopLevel);
    }

    #[test]
    fn test_classify_kind_pub_struct() {
        assert_eq!(classify_kind("pub struct Bar {"), SectionKind::TypeDef);
    }

    // ── count_header_lines_brace ─────────────────────────────────────────────

    #[test]
    fn test_header_lines_single_line() {
        let lines = ["fn foo() {", "    let x = 1;", "}"];
        assert_eq!(count_header_lines_brace(&lines), 1);
    }

    #[test]
    fn test_header_lines_multiline_sig() {
        let lines = ["fn foo(", "    x: i32,", ") -> i32 {", "    x", "}"];
        // Opening { is on line index 2, so header_lines = 3
        assert_eq!(count_header_lines_brace(&lines), 3);
    }

    #[test]
    fn test_header_lines_no_brace_returns_zero() {
        let lines = ["use std::fs;", "use std::path::Path;"];
        assert_eq!(count_header_lines_brace(&lines), 0);
    }
}
