use anyhow::Result;
use crate::handlers::{Handler, read::ReadHandlerLevel};
use panda_core::config::ReadMode;

pub fn run(file: &str, level: &str) -> Result<()> {
    let content = if file == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", file, e))?
    };

    let mode = match level.to_lowercase().as_str() {
        "auto"        => ReadMode::Auto,
        "strip"       => ReadMode::Strip,
        "aggressive"  => ReadMode::Aggressive,
        _             => ReadMode::Passthrough,
    };

    let handler = ReadHandlerLevel::from_read_mode(&mode);
    let args = if file == "-" {
        vec![]
    } else {
        vec![file.to_string()]
    };
    let filtered = handler.filter(&content, &args);

    let in_tok  = panda_core::tokens::count_tokens(&content);
    let out_tok = panda_core::tokens::count_tokens(&filtered);
    let saved   = in_tok.saturating_sub(out_tok);
    let pct     = if in_tok > 0 { saved * 100 / in_tok } else { 0 };

    print!("{}", filtered);

    eprintln!(
        "[panda read-file] level={} in={} out={} saved={}%",
        level, in_tok, out_tok, pct
    );
    Ok(())
}
