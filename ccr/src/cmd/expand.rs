use anyhow::Result;

pub fn run(id: &str, list: bool) -> Result<()> {
    if list {
        let ids = crate::zoom_store::list_blocks();
        if ids.is_empty() {
            println!("No zoom blocks available. Run a command through ccr first.");
        } else {
            for id in ids {
                println!("{}", id);
            }
        }
        return Ok(());
    }

    if id.is_empty() {
        anyhow::bail!("Usage: ccr expand <ZI_N>  or  ccr expand --list");
    }

    let content = crate::zoom_store::load_block(id)?;
    print!("{}", content);
    if !content.ends_with('\n') {
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_id_without_list_returns_error() {
        let result = super::run("", false);
        assert!(result.is_err());
    }
}
