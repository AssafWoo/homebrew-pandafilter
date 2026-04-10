use anyhow::Result;
use panda_core::pipeline::Pipeline;
use std::io::{self, Read, Write};

pub fn run(command_hint: Option<String>) -> Result<()> {
    let config = crate::config_loader::load_config()?;
    let pipeline = Pipeline::new(config);

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let result = pipeline.process(&input, command_hint.as_deref(), None, None)?;

    io::stdout().write_all(result.output.as_bytes())?;

    // Append analytics to SQLite (same path as ccr run / ccr hook)
    let project_path = crate::analytics_db::current_project_path();
    let _ = crate::analytics_db::append(&result.analytics, &project_path);

    Ok(())
}
