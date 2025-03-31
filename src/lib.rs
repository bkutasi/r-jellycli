//! r-jellycli library core functionality

pub mod audio;
pub mod config;
pub mod jellyfin;
pub mod player;
pub mod ui;

/// Initialize the application directories
pub fn init_app_dirs() -> std::io::Result<()> {
    let default_path = config::Settings::default_path();
    let config_dir = default_path.parent().unwrap();
    if !config_dir.exists() {
        std::fs::create_dir_all(config_dir)?;
    }
    Ok(())
}
