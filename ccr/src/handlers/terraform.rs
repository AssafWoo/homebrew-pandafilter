use super::util;
use super::Handler;

pub struct TerraformHandler;

impl Handler for TerraformHandler {
    fn rewrite_args(&self, args: &[String]) -> Vec<String> {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "plan" | "apply" | "destroy" => {
                if args.iter().any(|a| a == "-no-color" || a == "--no-color") {
                    args.to_vec()
                } else {
                    let mut out = args.to_vec();
                    out.push("-no-color".to_string());
                    out
                }
            }
            _ => args.to_vec(),
        }
    }

    fn filter(&self, output: &str, args: &[String]) -> String {
        let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
        match subcmd {
            "plan" => filter_plan(output),
            "apply" => filter_apply(output),
            "init" => filter_init(output),
            "validate" => filter_validate(output),
            "output" => filter_output(output),
            "state" => filter_state(output, args),
            _ => output.to_string(),
        }
    }
}

/// Cap a list of resource addresses at `limit`, appending "[+N more]" if needed.
fn cap_resources(resources: &[String], limit: usize) -> Vec<String> {
    if resources.len() <= limit {
        resources.to_vec()
    } else {
        let extra = resources.len() - limit;
        let mut out: Vec<String> = resources[..limit].to_vec();
        out.push(format!("[+{} more]", extra));
        out
    }
}

fn filter_plan(output: &str) -> String {
    const PLAN_NO_CHANGE_RULES: &[util::MatchOutputRule] = &[util::MatchOutputRule {
        success_pattern: r"No changes\. Your infrastructure matches the configuration",
        error_pattern: r"Error:",
        ok_message: "no changes detected",
    }];
    if let Some(msg) = util::check_match_output(output, PLAN_NO_CHANGE_RULES) {
        return msg;
    }

    // Pass through plain "No changes." outputs that don't match the full pattern above.
    if output.lines().any(|l| l.trim().starts_with("No changes.")) {
        return output.to_string();
    }

    // Parse resource-change lines: `  # <address> will be created/updated/destroyed/replaced`
    let mut creates: Vec<String> = Vec::new();
    let mut updates: Vec<String> = Vec::new();
    let mut destroys: Vec<String> = Vec::new();
    let mut replaces: Vec<String> = Vec::new();

    // Extract the terraform summary line ("Plan: N to add, M to change, K to destroy.")
    let mut plan_summary: Option<String> = None;

    for line in output.lines() {
        let t = line.trim();

        // Capture the official terraform summary line.
        if t.starts_with("Plan:") {
            plan_summary = Some(t.to_string());
            continue;
        }

        // Resource change annotations look like:  # module.foo.aws_s3_bucket.bar will be created
        if let Some(rest) = t.strip_prefix("# ") {
            if let Some(addr) = rest.strip_suffix(" will be created") {
                creates.push(addr.to_string());
            } else if rest.ends_with(" will be updated in-place") {
                let addr = rest.trim_end_matches(" will be updated in-place");
                updates.push(addr.to_string());
            } else if rest.ends_with(" will be destroyed") {
                let addr = rest.trim_end_matches(" will be destroyed");
                destroys.push(addr.to_string());
            } else if rest.ends_with(" must be replaced") {
                let addr = rest.trim_end_matches(" must be replaced");
                replaces.push(addr.to_string());
            }
        }
    }

    // If we found grouped resource changes, emit the compact grouped view.
    if !creates.is_empty() || !updates.is_empty() || !destroys.is_empty() || !replaces.is_empty() {
        let mut out: Vec<String> = Vec::new();

        // Header: prefer the terraform summary line; otherwise build one from counts.
        if let Some(summary) = plan_summary {
            out.push(format!("[{}]", summary));
        } else {
            out.push(format!(
                "[Plan: {} to add, {} to change, {} to destroy]",
                creates.len(),
                updates.len() + replaces.len(),
                destroys.len(),
            ));
        }

        const CAP: usize = 8;

        if !creates.is_empty() {
            out.push(format!("create ({}):", creates.len()));
            for r in cap_resources(&creates, CAP) {
                out.push(format!("  + {}", r));
            }
        }
        if !updates.is_empty() {
            out.push(format!("update ({}):", updates.len()));
            for r in cap_resources(&updates, CAP) {
                out.push(format!("  ~ {}", r));
            }
        }
        if !replaces.is_empty() {
            out.push(format!("replace ({}):", replaces.len()));
            for r in cap_resources(&replaces, CAP) {
                out.push(format!("  +/- {}", r));
            }
        }
        if !destroys.is_empty() {
            out.push(format!("destroy ({}):", destroys.len()));
            for r in cap_resources(&destroys, CAP) {
                out.push(format!("  - {}", r));
            }
        }

        return out.join("\n");
    }

    // Fallback: keep lines with diff markers or Plan:/No changes as before.
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with('+')
            || t.starts_with('-')
            || t.starts_with('~')
            || t.starts_with("Plan:")
            || t.contains("No changes")
        {
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_apply(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.contains(": Creating...")
            || t.contains(": Creation complete")
            || t.contains(": Destroying...")
            || t.contains(": Destruction complete")
            || t.contains(": Modifying...")
            || t.contains(": Modifications complete")
            || t.starts_with("Apply complete!")
            || t.contains("Error:")
            || t.starts_with("Error ")
        {
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_init(output: &str) -> String {
    let has_error = output.lines().any(|l| {
        let t = l.trim();
        t.starts_with("Error") || t.starts_with("error")
    });
    if has_error {
        let errors: Vec<&str> = output
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("Error") || t.starts_with("error") || t.contains("Error:")
            })
            .collect();
        return errors.join("\n");
    }
    "[terraform init complete]".to_string()
}

fn filter_validate(output: &str) -> String {
    const VALIDATE_OK_RULES: &[util::MatchOutputRule] = &[util::MatchOutputRule {
        success_pattern: r"(?i)The configuration is valid|Success!",
        error_pattern: r"(?i)error|Error",
        ok_message: "terraform validate: ok",
    }];
    if let Some(msg) = util::check_match_output(output, VALIDATE_OK_RULES) {
        return msg;
    }

    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.contains("Success")
            || t.contains("error")
            || t.contains("Error")
            || t.contains("warning")
        {
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        output.to_string()
    } else {
        out.join("\n")
    }
}

fn filter_output(output: &str) -> String {
    let mut pairs: Vec<String> = Vec::new();
    let mut current_key: Option<String> = None;
    let mut is_sensitive = false;

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() || t == "{" {
            continue;
        }
        if t == "}" {
            current_key = None;
            is_sensitive = false;
            continue;
        }
        if t == "sensitive = true" {
            if let Some(ref key) = current_key {
                pairs.push(format!("{} = <sensitive>", key));
            }
            current_key = None;
            is_sensitive = false;
            continue;
        }
        // Skip type/sensitive annotation lines inside a block
        if t.starts_with("type      =") || t.starts_with("sensitive =") {
            if t.contains("true") {
                is_sensitive = true;
            }
            continue;
        }
        if t.starts_with("value     =") || t.starts_with("value =") {
            let val_part = t.splitn(2, '=').nth(1).unwrap_or("").trim();
            if let Some(ref key) = current_key {
                if is_sensitive {
                    pairs.push(format!("{} = <sensitive>", key));
                } else {
                    pairs.push(format!("{} = {}", key, val_part));
                }
            }
            current_key = None;
            is_sensitive = false;
            continue;
        }
        // "key = value" or "key = {" pattern
        if let Some(eq_pos) = t.find(" = ") {
            let key = t[..eq_pos].trim();
            let val = t[eq_pos + 3..].trim();
            if val == "{" {
                current_key = Some(key.to_string());
            } else {
                pairs.push(format!("{} = {}", key, val));
            }
        }
    }

    if pairs.is_empty() { output.to_string() } else { pairs.join("\n") }
}

fn filter_state(output: &str, args: &[String]) -> String {
    let state_subcmd = args.get(2).map(|s| s.as_str()).unwrap_or("");
    match state_subcmd {
        "list" => {
            let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
            const MAX: usize = 50;
            if lines.len() > MAX {
                let extra = lines.len() - MAX;
                let mut out: Vec<String> = lines[..MAX].iter().map(|l| l.to_string()).collect();
                out.push(format!("[+{} more]", extra));
                out.join("\n")
            } else {
                lines.join("\n")
            }
        }
        "show" => {
            const KEEP_ATTRS: &[&str] = &["id", "name", "type", "status", "arn", "region"];
            let mut out: Vec<String> = Vec::new();
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() { continue; }
                if t.starts_with('#') || t.starts_with("resource ") || t == "{" || t == "}" {
                    out.push(line.to_string());
                    continue;
                }
                if let Some(eq_pos) = t.find(" = ") {
                    let attr = t[..eq_pos].trim().trim_matches('"');
                    if KEEP_ATTRS.iter().any(|k| {
                        attr == *k || attr.ends_with(&format!("_{}", k)) || attr.starts_with(&format!("{}_", k))
                    }) {
                        out.push(line.to_string());
                    }
                }
            }
            if out.is_empty() { output.to_string() } else { out.join("\n") }
        }
        _ => output.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler() -> TerraformHandler {
        TerraformHandler
    }

    #[test]
    fn plan_no_changes_short_circuits() {
        let output = "Refreshing state...\nNo changes. Your infrastructure matches the configuration.\nTerraform has compared your real infrastructure against your configuration\nand found no differences, so no changes are needed.";
        let result = handler().filter(output, &["terraform".to_string(), "plan".to_string()]);
        assert_eq!(result, "no changes detected");
    }

    #[test]
    fn plan_with_error_not_short_circuited() {
        let output = "No changes. Your infrastructure matches the configuration.\nError: Invalid resource configuration";
        let result = handler().filter(output, &["terraform".to_string(), "plan".to_string()]);
        assert_ne!(result, "no changes detected");
    }

    #[test]
    fn validate_ok_short_circuits() {
        let output = "Success! The configuration is valid.\n";
        let result = handler().filter(output, &["terraform".to_string(), "validate".to_string()]);
        assert_eq!(result, "terraform validate: ok");
    }

    #[test]
    fn test_output_compact() {
        let output = "db_endpoint = \"rds.example.com\"\nbucket_name = \"my-bucket\"\n";
        let result = handler().filter(output, &["terraform".to_string(), "output".to_string()]);
        assert!(result.contains("db_endpoint"), "got: {}", result);
        assert!(result.contains("bucket_name"), "got: {}", result);
    }

    #[test]
    fn test_output_sensitive_redacted() {
        let output = "db_password = {\n  sensitive = true\n  value     = \"supersecret\"\n  type      = \"string\"\n}\n";
        let result = handler().filter(output, &["terraform".to_string(), "output".to_string()]);
        assert!(result.contains("<sensitive>"), "got: {}", result);
        assert!(!result.contains("supersecret"), "got: {}", result);
    }

    #[test]
    fn plan_groups_resources_by_action() {
        // 5 creates, 3 updates, 1 destroy — must be grouped with correct header
        let mut input = String::new();
        input.push_str("Terraform will perform the following actions:\n\n");
        for i in 1..=5 {
            input.push_str(&format!("  # aws_iam_role.role_{} will be created\n", i));
            input.push_str("  + resource \"aws_iam_role\" \"role_1\" {\n  }\n\n");
        }
        for i in 1..=3 {
            input.push_str(&format!(
                "  # aws_lambda_function.fn_{} will be updated in-place\n",
                i
            ));
            input.push_str("  ~ resource \"aws_lambda_function\" \"fn_1\" {\n  }\n\n");
        }
        input.push_str("  # aws_s3_bucket.old will be destroyed\n");
        input.push_str("  - resource \"aws_s3_bucket\" \"old\" {\n  }\n\n");
        input.push_str("Plan: 5 to add, 3 to change, 1 to destroy.\n");

        let result = handler().filter(&input, &["terraform".to_string(), "plan".to_string()]);

        // Header present
        assert!(result.contains("Plan: 5 to add"), "missing header, got: {}", result);
        // Groups present
        assert!(result.contains("create (5):"), "missing create group, got: {}", result);
        assert!(result.contains("update (3):"), "missing update group, got: {}", result);
        assert!(result.contains("destroy (1):"), "missing destroy group, got: {}", result);
        // Spot-check one resource address per group
        assert!(result.contains("aws_iam_role.role_1"), "got: {}", result);
        assert!(result.contains("aws_lambda_function.fn_1"), "got: {}", result);
        assert!(result.contains("aws_s3_bucket.old"), "got: {}", result);
    }

    #[test]
    fn plan_no_changes_passthrough() {
        // Plain "No changes." output must pass through unchanged
        let input = "No changes. Your infrastructure is up-to-date.\n\
                     Terraform has compared your state and found no differences.\n";
        let result = handler().filter(input, &["terraform".to_string(), "plan".to_string()]);
        // Either the check_match_output short-circuit fires or the passthrough returns the full text.
        // Either way the key phrase must be present.
        assert!(
            result.contains("No changes") || result.contains("no changes"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_state_list_capped() {
        let mut output = String::new();
        for i in 0..60 {
            output.push_str(&format!("aws_instance.web_{}\n", i));
        }
        let result = handler().filter(
            &output,
            &["terraform".to_string(), "state".to_string(), "list".to_string()],
        );
        assert!(result.contains("[+10 more]"), "got: {}", result);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 51, "should have 50 resources + 1 overflow line, got {}", lines.len());
    }
}
