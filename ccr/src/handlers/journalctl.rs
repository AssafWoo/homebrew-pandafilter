use super::Handler;

pub struct JournalctlHandler;

impl Handler for JournalctlHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let mut out = args.to_vec();
        if !args.iter().any(|a| a == "--no-pager") {
            out.push("--no-pager".to_string());
        }
        if !args.iter().any(|a| a == "-n" || a.starts_with("--lines")) {
            out.push("-n".to_string());
            out.push("200".to_string());
        }
        out
    }

    fn filter(&self, output: &str, _args: &[String]) -> String {
        let lines_in = output.lines().count();
        if lines_in == 0 {
            return output.to_string();
        }
        // Anomaly scoring: errors and unique events score highest, routine noise lowest.
        let budget = (lines_in / 3).max(20).min(200);
        panda_core::summarizer::summarize(output, budget).output
    }
}
