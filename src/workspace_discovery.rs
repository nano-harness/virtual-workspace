use anyhow::Result;
use std::path::PathBuf;
use vwsd::model::Workspace;

pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".vws")
        .join("workspaces")
}

pub fn list_configs() -> Vec<(PathBuf, Workspace)> {
    let dir = config_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut configs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(mut ws) = serde_json::from_str::<Workspace>(&text) {
                ws.expand_paths();
                configs.push((path, ws));
            }
        }
    }
    configs.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    configs
}

pub fn find_config_by_name(name: &str) -> Option<(PathBuf, Workspace)> {
    list_configs().into_iter().find(|(_, ws)| ws.name == name)
}

pub fn select_workspace(prompt: &str) -> Result<Option<(PathBuf, Workspace)>> {
    let all = list_configs();
    if all.is_empty() {
        return Ok(None);
    }
    let labels: Vec<String> = all
        .iter()
        .map(|(path, ws)| format!("{:<20} {}", ws.name, path.display()))
        .collect();
    let selection = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&labels)
        .default(0)
        .interact_opt()?;
    Ok(selection.map(|index| all.into_iter().nth(index).unwrap()))
}
