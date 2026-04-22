use serde::Deserialize;

#[derive(Deserialize)]
pub struct AccountInfo {
    pub pubkey: String,
    pub balance: u64,
    pub lichen: f64,
    pub exists: bool,
    pub is_executable: bool,
    pub is_validator: bool,
}
