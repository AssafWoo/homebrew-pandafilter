//! Output assembler — build the guidance block from query results.

use crate::focus::query::RankedFile;
use serde::{Deserialize, Serialize};

/// Guidance output: recommended files, excluded files, negative guidance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuidanceOutput {
    pub recommended_files: Vec<FileEntry>,
    pub negative_guidance: Vec<String>,
    pub guidance_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub role: String,
    pub confidence: f64,
}

/// Assemble guidance from query results.
///
/// Builds a formatted guidance block that tells Claude:
/// 1. Which files to focus on (recommended)
/// 2. Which files to explicitly avoid (negative guidance)
pub fn assemble(
    recommended: Vec<RankedFile>,
    _total_files: usize,
) -> GuidanceOutput {
    // Convert to display format
    let recommended_files: Vec<FileEntry> = recommended
        .iter()
        .map(|r| FileEntry {
            path: r.path.clone(),
            role: r.role.clone(),
            confidence: r.confidence,
        })
        .collect();

    // Build negative guidance: identify exclusion patterns
    let negative_guidance = build_negative_guidance(&recommended_files);

    // Format as human-readable text
    let guidance_text = format_guidance(&recommended_files);

    GuidanceOutput {
        recommended_files,
        negative_guidance,
        guidance_text,
    }
}

/// Build negative guidance — intentionally empty.
///
/// Exclusion lists are not emitted: telling the agent to skip files it hasn't
/// read creates a hard miss when the ranking is wrong (~14% of queries). Ranked
/// recommendations already surface the most likely files; the agent discovers
/// the rest naturally if the hints don't pan out.
fn build_negative_guidance(_recommended: &[FileEntry]) -> Vec<String> {
    vec![]
}

/// Format guidance as human-readable text for injection into context.
fn format_guidance(recommended: &[FileEntry]) -> String {
    if recommended.is_empty() {
        return "## Context Focus\nNo specific files identified as most relevant.".to_string();
    }

    let mut text = String::from("## Context Focus\nMost likely relevant files (all files remain accessible):\n");

    for (i, file) in recommended.iter().take(6).enumerate() {
        // Format role for display
        let role_display = match file.role.as_str() {
            "entry_point" => "Entry Point",
            "persistence" => "Database/Storage",
            "state_manager" => "State",
            "validator" => "Validation",
            "test" => "Test",
            _ => "Other",
        };

        text.push_str(&format!(
            "{}. `{}` [{}]\n",
            i + 1,
            file.path,
            role_display
        ));
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_guidance_empty() {
        let text = format_guidance(&[]);
        assert!(text.contains("No specific files"));
    }

    #[test]
    fn test_format_guidance_with_files() {
        let files = vec![
            FileEntry {
                path: "src/main.rs".to_string(),
                role: "entry_point".to_string(),
                confidence: 0.95,
            },
            FileEntry {
                path: "src/db.rs".to_string(),
                role: "persistence".to_string(),
                confidence: 0.87,
            },
        ];

        let text = format_guidance(&files);
        assert!(text.contains("src/main.rs"));
        assert!(text.contains("Entry Point"));
        assert!(text.contains("src/db.rs"));
        assert!(text.contains("Database/Storage"));
        assert!(text.contains("all files remain accessible"));
    }

    #[test]
    fn test_assemble_creates_output() {
        let ranked = vec![RankedFile {
            path: "src/main.rs".to_string(),
            role: "entry_point".to_string(),
            confidence: 0.95,
            cochange_count: 5,
            relevance_score: 0.92,
        }];

        let output = assemble(ranked, 100);
        assert_eq!(output.recommended_files.len(), 1);
        assert!(!output.guidance_text.is_empty());
    }
}
