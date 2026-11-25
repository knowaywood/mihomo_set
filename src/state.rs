use anyhow::{Context, Result};
use std::path::PathBuf;

fn state_path() -> Result<PathBuf> {
    let base = match std::env::var("XDG_STATE_HOME") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
            .join(".local/state"),
    };
    let dir = base.join("mset");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create state dir {}", dir.display()))?;
    Ok(dir.join("last_test.json"))
}

pub fn save_ranking(nodes: &[String]) -> Result<()> {
    let json = serde_json::json!({ "nodes": nodes });
    std::fs::write(state_path()?, json.to_string().as_bytes())
        .context("failed to save test ranking")?;
    Ok(())
}

pub fn load_ranking() -> Option<Vec<String>> {
    let p = state_path().ok()?;
    let text = std::fs::read_to_string(p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let arr = v.get("nodes")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
    )
}
