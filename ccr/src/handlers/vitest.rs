use super::util;
use super::Handler;

pub struct VitestHandler;

impl Handler for VitestHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Force JSON output for deterministic, structured results.
        // Remove any existing --reporter flag first to avoid conflicts.
        out.retain(|a| !a.starts_with("--reporter"));
        out.push("--reporter=json".to_string());
        // Force single-run mode (non-watch) for CI/automation contexts.
        if !out.iter().any(|a| a == "run" || a == "--run") {
            // Insert "run" after the binary name (args[0] = "vitest")
            let insert_pos = 1.min(out.len());
            out.insert(insert_pos, "run".to_string());
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        util::test_failures(output, "vitest")
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
    fn injects_json_reporter() {
        let result = VitestHandler.rewrite_args(&args(&["vitest"]));
        assert!(result.contains(&"--reporter=json".to_string()), "should inject --reporter=json");
    }

    #[test]
    fn replaces_verbose_reporter_with_json() {
        let result = VitestHandler.rewrite_args(&args(&["vitest", "--reporter=verbose"]));
        assert!(result.contains(&"--reporter=json".to_string()), "should replace with json");
        let verbose_count = result.iter().filter(|a| a.contains("verbose")).count();
        assert_eq!(verbose_count, 0, "should not have verbose reporter");
    }

    #[test]
    fn injects_run_mode() {
        let result = VitestHandler.rewrite_args(&args(&["vitest"]));
        assert!(result.contains(&"run".to_string()), "should inject run mode");
    }

    #[test]
    fn does_not_double_run_mode() {
        let result = VitestHandler.rewrite_args(&args(&["vitest", "run"]));
        let run_count = result.iter().filter(|a| a.as_str() == "run").count();
        assert_eq!(run_count, 1, "should not inject duplicate run");
    }

    #[test]
    fn json_all_passing_compact() {
        let json = r#"{"numTotalTests":13,"numPassedTests":13,"numFailedTests":0,"numPendingTests":0,"testResults":[],"startTime":1000}"#;
        let result = VitestHandler.filter(json, &args(&["vitest"]));
        assert_eq!(result, "Tests: 13 passed (13)");
    }

    #[test]
    fn json_with_failures() {
        let json = r#"{"numTotalTests":5,"numPassedTests":4,"numFailedTests":1,"numPendingTests":0,"testResults":[{"name":"foo.test.ts","assertionResults":[{"fullName":"foo fails","status":"failed","failureMessages":["Expected true"]}]}],"startTime":1000}"#;
        let result = VitestHandler.filter(json, &args(&["vitest"]));
        assert!(result.contains("1 FAILED"), "should show failure count");
        assert!(result.contains("foo fails"), "should show failure name");
    }

    #[test]
    fn pnpm_prefix_json_extracted() {
        let input = "Scope: all 3 workspace projects\nWARN deprecated inflight@1.0.6\n{\"numTotalTests\":5,\"numPassedTests\":5,\"numFailedTests\":0,\"numPendingTests\":0,\"testResults\":[],\"startTime\":1000}";
        let result = VitestHandler.filter(input, &args(&["vitest"]));
        assert_eq!(result, "Tests: 5 passed (5)");
    }
}
