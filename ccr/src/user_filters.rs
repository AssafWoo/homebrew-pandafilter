use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use crate::handlers::Handler;

/// Compile a user-supplied pattern as a regex.
/// Falls back to a literal (escaped) match if the pattern is not valid regex.
fn compile_pattern(pattern: &str) -> Regex {
    Regex::new(pattern)
        .unwrap_or_else(|_| Regex::new(&regex::escape(pattern)).expect("escaped pattern must be valid"))
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserFiltersFile {
    #[serde(default)]
    pub commands: HashMap<String, UserCommandFilter>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserCommandFilter {
    #[serde(default)]
    pub strip_lines_matching: Vec<String>,
    #[serde(default)]
    pub keep_lines_matching: Vec<String>,
    pub match_output: Option<UserMatchOutput>,
    pub on_empty: Option<String>,
    pub max_lines: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserMatchOutput {
    pub pattern: String,
    pub message: String,
    pub unless_pattern: Option<String>,
}

/// Load user filter definitions from global config and project-local config.
/// Project-local overrides global for the same command keys.
pub fn load_user_filters() -> UserFiltersFile {
    let mut merged = UserFiltersFile::default();

    // 1. Try global config: ~/.config/panda/filters.toml
    if let Some(config_dir) = dirs::config_dir() {
        let global_path = config_dir.join("panda").join("filters.toml");
        if let Ok(contents) = std::fs::read_to_string(&global_path) {
            if let Ok(parsed) = toml::from_str::<UserFiltersFile>(&contents) {
                for (k, v) in parsed.commands {
                    merged.commands.insert(k, v);
                }
            }
        }
    }

    // 2. Try project-local: .panda/filters.toml (from cwd)
    if let Ok(cwd) = std::env::current_dir() {
        let local_path = cwd.join(".panda").join("filters.toml");
        if let Ok(contents) = std::fs::read_to_string(&local_path) {
            if let Ok(parsed) = toml::from_str::<UserFiltersFile>(&contents) {
                for (k, v) in parsed.commands {
                    // Project-local overrides global
                    merged.commands.insert(k, v);
                }
            }
        }
    }

    merged
}

pub struct UserFilterHandler {
    pub filter_def: UserCommandFilter,
    strip_regexes: Vec<Regex>,
    keep_regexes: Vec<Regex>,
    match_output_regex: Option<Regex>,
    unless_regex: Option<Regex>,
}

impl UserFilterHandler {
    pub fn new(filter_def: UserCommandFilter) -> Self {
        let strip_regexes = filter_def
            .strip_lines_matching
            .iter()
            .map(|p| compile_pattern(p))
            .collect();
        let keep_regexes = filter_def
            .keep_lines_matching
            .iter()
            .map(|p| compile_pattern(p))
            .collect();
        let match_output_regex = filter_def
            .match_output
            .as_ref()
            .map(|mo| compile_pattern(&mo.pattern));
        let unless_regex = filter_def
            .match_output
            .as_ref()
            .and_then(|mo| mo.unless_pattern.as_ref())
            .map(|p| compile_pattern(p));
        Self {
            filter_def,
            strip_regexes,
            keep_regexes,
            match_output_regex,
            unless_regex,
        }
    }
}

impl Handler for UserFilterHandler {
    fn filter(&self, output: &str, _args: &[String]) -> String {
        let def = &self.filter_def;

        // 1. Check match_output short-circuit
        if let Some(mo) = &def.match_output {
            if let Some(re) = &self.match_output_regex {
                let pattern_matches = re.is_match(output);
                let unless_blocks = self
                    .unless_regex
                    .as_ref()
                    .map(|r| r.is_match(output))
                    .unwrap_or(false);
                if pattern_matches && !unless_blocks {
                    return mo.message.clone();
                }
            }
        }

        // 2. Apply strip_lines_matching
        let mut lines: Vec<&str> = output.lines().collect();
        if !self.strip_regexes.is_empty() {
            lines.retain(|line| !self.strip_regexes.iter().any(|re| re.is_match(line)));
        }

        // 3. Apply keep_lines_matching
        if !self.keep_regexes.is_empty() {
            lines.retain(|line| self.keep_regexes.iter().any(|re| re.is_match(line)));
        }

        // 4. Apply max_lines cap
        if let Some(max) = def.max_lines {
            lines.truncate(max);
        }

        // 5. If result is empty, return on_empty
        if lines.is_empty() {
            return def.on_empty.clone().unwrap_or_default();
        }

        // 6. Return filtered output
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handler(def: UserCommandFilter) -> UserFilterHandler {
        UserFilterHandler::new(def)
    }

    #[test]
    fn test_strip_lines_matching_removes_correct_lines() {
        let def = UserCommandFilter {
            strip_lines_matching: vec!["WARNING".to_string(), "DEBUG".to_string()],
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "INFO: ok\nWARNING: something\nDEBUG: trace\nINFO: done";
        let result = handler.filter(output, &[]);
        assert!(result.contains("INFO: ok"));
        assert!(result.contains("INFO: done"));
        assert!(!result.contains("WARNING"));
        assert!(!result.contains("DEBUG"));
    }

    #[test]
    fn test_keep_lines_matching_keeps_only_matching() {
        let def = UserCommandFilter {
            keep_lines_matching: vec!["ERROR".to_string()],
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "INFO: ok\nERROR: something bad\nINFO: done\nERROR: another";
        let result = handler.filter(output, &[]);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.contains("ERROR")));
    }

    #[test]
    fn test_match_output_fires_when_pattern_matches() {
        let def = UserCommandFilter {
            match_output: Some(UserMatchOutput {
                pattern: "Build succeeded".to_string(),
                message: "✓ build ok".to_string(),
                unless_pattern: None,
            }),
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "Compiling...\nBuild succeeded\nFinished in 2s";
        let result = handler.filter(output, &[]);
        assert_eq!(result, "✓ build ok");
    }

    #[test]
    fn test_match_output_blocked_by_unless_pattern() {
        let def = UserCommandFilter {
            match_output: Some(UserMatchOutput {
                pattern: "Build succeeded".to_string(),
                message: "✓ build ok".to_string(),
                unless_pattern: Some("with warnings".to_string()),
            }),
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "Build succeeded\nFinished with warnings";
        let result = handler.filter(output, &[]);
        // unless_pattern matches → short-circuit should NOT fire
        assert_ne!(result, "✓ build ok");
        assert!(result.contains("Build succeeded"));
    }

    #[test]
    fn test_on_empty_returned_when_all_filtered() {
        let def = UserCommandFilter {
            strip_lines_matching: vec!["noise".to_string()],
            on_empty: Some("(no output)".to_string()),
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "noise\nnoise again\nmore noise";
        let result = handler.filter(output, &[]);
        assert_eq!(result, "(no output)");
    }

    #[test]
    fn test_on_empty_default_when_not_set() {
        let def = UserCommandFilter {
            strip_lines_matching: vec!["everything".to_string()],
            on_empty: None,
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "everything goes away";
        let result = handler.filter(output, &[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_max_lines_caps_output() {
        let def = UserCommandFilter {
            max_lines: Some(3),
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "line1\nline2\nline3\nline4\nline5";
        let result = handler.filter(output, &[]);
        assert_eq!(result.lines().count(), 3);
        assert!(result.contains("line1"));
        assert!(!result.contains("line4"));
    }

    #[test]
    fn test_strip_then_keep_combined() {
        let def = UserCommandFilter {
            strip_lines_matching: vec!["noise".to_string()],
            keep_lines_matching: vec!["important".to_string()],
            ..Default::default()
        };
        let handler = make_handler(def);
        let output = "important: yes\nnoise: skip\nimportant noise: tricky\nother: skip";
        let result = handler.filter(output, &[]);
        // "important noise: tricky" is first stripped (contains "noise"), then keep_lines checks survivors
        // After strip: ["important: yes", "other: skip"]
        // After keep:  ["important: yes"]
        assert!(result.contains("important: yes"));
        assert!(!result.contains("noise"));
        assert!(!result.contains("other"));
    }
}
