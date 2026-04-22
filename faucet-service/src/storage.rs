use std::{fs, path::Path};

use super::models::AirdropRecord;

pub(super) fn load_airdrops(path: &str) -> Vec<AirdropRecord> {
    if !Path::new(path).exists() {
        return Vec::new();
    }

    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub(super) fn save_airdrops(path: &str, records: &[AirdropRecord]) -> Result<(), String> {
    let parent = Path::new(path).parent().map(|value| value.to_path_buf());
    if let Some(parent) = parent {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    let payload = serde_json::to_vec_pretty(records).map_err(|err| err.to_string())?;
    fs::write(path, payload).map_err(|err| err.to_string())
}
