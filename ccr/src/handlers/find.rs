use super::Handler;
use std::collections::BTreeMap;

pub struct FindHandler;

impl Handler for FindHandler {
    fn skip_bert(&self) -> bool {
        // find output is a structured directory tree; BERT may corrupt the grouping.
        true
    }

    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Inject -maxdepth 8 if no depth limit is already set
        // Prevents runaway traversal into deeply nested node_modules / .git trees
        if !out.iter().any(|a| a == "-maxdepth" || a == "-mindepth") {
            // Insert after the path argument (index 1) if present, else append
            if out.len() >= 2 {
                out.insert(2, "8".to_string());
                out.insert(2, "-maxdepth".to_string());
            } else {
                out.push("-maxdepth".to_string());
                out.push("8".to_string());
            }
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        // Short-circuit: output is already a count (piped through `wc -l`).
        // A single integer line means there is nothing left to compress.
        if output.trim().parse::<u64>().is_ok() {
            return output.to_string();
        }

        let lines: Vec<&str> = output
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();

        let total = lines.len();
        if total <= 50 {
            return output.to_string();
        }

        // Find common prefix to strip
        let prefix = common_prefix(&lines);

        // Group by parent directory
        let mut by_dir: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for line in &lines {
            let stripped = if !prefix.is_empty() && line.starts_with(&prefix) {
                &line[prefix.len()..]
            } else {
                line
            };

            let parent = parent_dir(stripped);
            by_dir
                .entry(parent.to_string())
                .or_default()
                .push(stripped.to_string());
        }

        let mut out: Vec<String> = Vec::new();
        let mut shown = 0;
        const LIMIT: usize = 50;

        if !prefix.is_empty() {
            out.push(format!("[root: {}]", prefix.trim_end_matches('/')));
        }

        'outer: for (dir, files) in &by_dir {
            out.push(format!("{}/ ({} entries)", dir, files.len()));
            for f in files.iter().take(5) {
                if shown >= LIMIT {
                    break 'outer;
                }
                // Show just the filename, not the full path
                let name = f.rsplit('/').next().unwrap_or(f);
                out.push(format!("  {}", name));
                shown += 1;
            }
            if files.len() > 5 {
                out.push(format!("  [+{} more]", files.len() - 5));
            }
        }

        out.push(format!("[{} total, {} dirs]", total, by_dir.len()));
        out.join("\n")
    }
}

fn common_prefix(lines: &[&str]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let first = lines[0];
    // Find the longest common path prefix
    let mut prefix_end = 0;
    for (i, c) in first.char_indices() {
        let sub = &first[..i + c.len_utf8()];
        if lines.iter().all(|l| l.starts_with(sub)) {
            prefix_end = i + c.len_utf8();
        } else {
            break;
        }
    }
    // Trim to last '/'
    let prefix = &first[..prefix_end];
    if let Some(pos) = prefix.rfind('/') {
        first[..pos + 1].to_string()
    } else {
        String::new()
    }
}

fn parent_dir(path: &str) -> &str {
    if let Some(pos) = path.rfind('/') {
        if pos == 0 {
            "/"
        } else {
            &path[..pos]
        }
    } else {
        "."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_passthrough_single_integer_wc_l() {
        // Output is the result of `find ... | wc -l` — already maximally compact.
        let handler = FindHandler;
        assert_eq!(handler.filter("127\n", &[]), "127\n");
        assert_eq!(handler.filter("0\n", &[]), "0\n");
        assert_eq!(handler.filter("   42   ", &[]), "   42   ");
    }

    #[test]
    fn filter_applies_path_filtering_for_many_paths() {
        // Build 60 file paths (>50 threshold) across five directories.
        // This exceeds the passthrough limit and triggers the grouped summary.
        let paths: String = (0..60)
            .map(|i| format!("/project/src/module_{}/file_{}.rs\n", i % 5, i))
            .collect();
        let handler = FindHandler;
        let result = handler.filter(&paths, &[]);
        // Should not be a passthrough: the grouped summary must appear.
        assert!(
            result.contains("entries") || result.contains("total"),
            "expected grouped output, got: {:?}",
            result
        );
        // The handler groups by directory and shows at most 5 per directory,
        // so output should be significantly shorter than 60 raw lines.
        let raw_line_count = result.lines().count();
        assert!(
            raw_line_count < 60,
            "expected fewer than 60 lines after compression, got {}",
            raw_line_count
        );
    }
}
