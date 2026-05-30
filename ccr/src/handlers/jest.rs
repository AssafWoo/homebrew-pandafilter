use super::Handler;
use super::util;

pub struct JestHandler;

impl Handler for JestHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Force JSON output for deterministic, parseable results.
        if !out.iter().any(|a| a == "--json") {
            out.push("--json".to_string());
        }
        // Suppress the verbose coverage table — it's large noise when checking for failures.
        if !out.iter().any(|a| a == "--no-coverage" || a == "--coverage") {
            out.push("--no-coverage".to_string());
        }
        // Force non-watch mode for automation contexts.
        if !out.iter().any(|a| a == "--no-watch" || a == "--watchAll=false") {
            out.push("--no-watch".to_string());
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        util::test_failures(output, "jest")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::Handler;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn injects_json_flag() {
        let result = JestHandler.rewrite_args(&args(&["jest"]));
        assert!(result.contains(&"--json".to_string()), "should inject --json");
    }

    #[test]
    fn does_not_double_json() {
        let result = JestHandler.rewrite_args(&args(&["jest", "--json"]));
        let count = result.iter().filter(|a| a.as_str() == "--json").count();
        assert_eq!(count, 1, "should not double --json");
    }

    #[test]
    fn injects_no_coverage() {
        let result = JestHandler.rewrite_args(&args(&["jest"]));
        assert!(result.contains(&"--no-coverage".to_string()));
    }

    #[test]
    fn injects_no_watch() {
        let result = JestHandler.rewrite_args(&args(&["jest"]));
        assert!(result.contains(&"--no-watch".to_string()));
    }

    #[test]
    fn json_all_passing_compact() {
        let json = r#"{"numTotalTests":13,"numPassedTests":13,"numFailedTests":0,"numPendingTests":0,"testResults":[],"startTime":1000}"#;
        let result = JestHandler.filter(json, &args(&["jest"]));
        assert_eq!(result, "Tests: 13 passed (13)");
    }

    #[test]
    fn json_with_failures() {
        let json = r#"{"numTotalTests":5,"numPassedTests":4,"numFailedTests":1,"numPendingTests":0,"testResults":[{"testFilePath":"auth.test.js","testResults":[{"fullName":"auth login fails","status":"failed","failureMessages":["Expected true"]}]}],"startTime":1000}"#;
        let result = JestHandler.filter(json, &args(&["jest"]));
        assert!(result.contains("1 FAILED"), "should show failure count: {}", result);
        assert!(result.contains("auth login fails"), "should show failure name: {}", result);
    }

    #[test]
    fn text_fallback_extracts_summary() {
        let output = "\
 PASS  src/auth.test.js\n\
Test Suites: 1 passed, 1 total\n\
Tests:       5 passed, 5 total\n\
Time:        1.234 s\n";
        let result = JestHandler.filter(output, &args(&["jest"]));
        assert!(result.contains("Tests:"), "should keep Tests: line: {}", result);
        assert!(!result.contains("PASS "), "should drop PASS lines: {}", result);
    }
}
