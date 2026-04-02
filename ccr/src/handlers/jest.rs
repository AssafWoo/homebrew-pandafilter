use super::Handler;
use super::util;

pub struct JestHandler;

impl Handler for JestHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        // Suppress the verbose coverage table — it's noise when checking for failures
        if !out.iter().any(|a| a == "--no-coverage" || a == "--coverage") {
            out.push("--no-coverage".to_string());
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        util::test_failures(output, "jest")
    }
}
