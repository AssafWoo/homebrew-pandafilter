use anyhow::Result;
use panda_core::pipeline::Pipeline;
use std::io::{self, Read, Write};

fn try_handler(input: &str, hint: &str) -> Option<String> {
    let parts: Vec<&str> = hint.split(|c: char| c == '-' || c == ' ').collect();
    let base_cmd = parts.first().copied().unwrap_or(hint);
    let handler = crate::handlers::get_handler(base_cmd)?;

    let args: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
    let filtered = handler.filter(input, &args);

    if filtered == input {
        return None;
    }
    Some(filtered)
}

pub fn run(command_hint: Option<String>) -> Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let (output, subcommand) = if let Some(ref hint) = command_hint {
        if let Some(filtered) = try_handler(&input, hint) {
            let sub = hint.split(|c: char| c == '-' || c == ' ').next().map(|s| s.to_string());
            (filtered, sub)
        } else {
            let config = crate::config_loader::load_config()?;
            let pipeline = Pipeline::new(config);
            let result = pipeline.process(&input, command_hint.as_deref(), None, None)?;
            (result.output, None)
        }
    } else {
        let config = crate::config_loader::load_config()?;
        let pipeline = Pipeline::new(config);
        let result = pipeline.process(&input, None, None, None)?;
        (result.output, None)
    };

    io::stdout().write_all(output.as_bytes())?;

    let input_tokens = panda_core::tokens::count_tokens(&input);
    let output_tokens = panda_core::tokens::count_tokens(&output);
    let analytics = panda_core::analytics::Analytics::new(
        input_tokens, output_tokens, command_hint.clone(), subcommand, None,
    );
    let project_path = crate::analytics_db::current_project_path();
    let _ = crate::analytics_db::append(&analytics, &project_path);

    Ok(())
}
