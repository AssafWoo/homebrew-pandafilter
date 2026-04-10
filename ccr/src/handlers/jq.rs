use super::util;
use super::Handler;

pub struct JqHandler;

impl Handler for JqHandler {
    fn filter(&self, output: &str, _args: &[String]) -> String {
        let trimmed = output.trim();
        let lines: Vec<&str> = trimmed.lines().collect();

        if lines.len() <= 20 {
            return output.to_string();
        }

        // JSON array output
        if trimmed.starts_with('[') {
            if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(trimmed) {
                if !arr.is_empty() {
                    let schema = util::json_to_schema(&arr[0]);
                    let schema_str = serde_json::to_string_pretty(&schema).unwrap_or_default();
                    return format!("{}\n[{} items]", schema_str, arr.len());
                }
                return "[empty array]".to_string();
            }
        }

        // JSON object output
        if trimmed.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                let schema = util::json_to_schema(&v);
                let schema_str = serde_json::to_string_pretty(&schema).unwrap_or_default();
                if schema_str.len() < trimmed.len() {
                    return schema_str;
                }
            }
        }

        // Plain text output: head+tail or BERT
        let n = lines.len();
        if n <= 500 {
            let head = &lines[..60.min(n)];
            let tail = &lines[n.saturating_sub(20)..];
            let omitted = n.saturating_sub(80);
            if omitted > 0 {
                let mut out: Vec<String> = head.iter().map(|l| l.to_string()).collect();
                out.push(format!("[... {} lines omitted ...]", omitted));
                out.extend(tail.iter().map(|l| l.to_string()));
                return out.join("\n");
            }
        }

        let result = panda_core::summarizer::summarize(output, 40);
        result.output
    }
}
