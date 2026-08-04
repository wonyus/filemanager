#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::command;

use crate::domain::error::PublicError;

fn validate_path(value: &str, label: &str) -> Result<PathBuf, PublicError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.chars().any(char::is_control) {
        return Err(PublicError::from(
            crate::domain::error::AppError::Validation(format!("{label} path is invalid")),
        ));
    }
    Ok(PathBuf::from(trimmed))
}

/// Open the native Windows file picker.  The returned path is only a user
/// selection; the transfer command still validates and authorizes it before
/// reading or writing any bytes.
#[command]
pub fn pick_file() -> Option<String> {
    rfd::FileDialog::new()
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

#[command]
pub fn pick_directory() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned())
}

#[command]
pub fn pick_save_file(default_name: Option<String>) -> Option<String> {
    let dialog = match default_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => rfd::FileDialog::new().set_file_name(value),
        None => rfd::FileDialog::new(),
    };
    dialog
        .save_file()
        .map(|path| path.to_string_lossy().into_owned())
}

/// Open an existing destination directory in the platform file manager.
/// Paths are passed as arguments (never through a shell) to avoid command
/// injection from object names or user-entered paths.
#[command]
pub fn open_destination_folder(path: String) -> Result<(), PublicError> {
    let path = validate_path(&path, "destination")?;
    let directory = if path.is_dir() {
        path
    } else {
        path.parent().map(Path::to_path_buf).ok_or_else(|| {
            PublicError::from(crate::domain::error::AppError::Validation(
                "destination has no parent directory".to_string(),
            ))
        })?
    };
    let directory = std::fs::canonicalize(&directory)
        .map_err(|error| PublicError::from(crate::domain::error::AppError::Io(error)))?;
    if !directory.is_dir() {
        return Err(PublicError::from(
            crate::domain::error::AppError::Validation(
                "destination directory does not exist".to_string(),
            ),
        ));
    }

    #[cfg(target_os = "windows")]
    let result = Command::new("explorer.exe")
        .arg(&directory)
        .spawn()
        .map(|_| ());
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(&directory).spawn().map(|_| ());
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(&directory).spawn().map(|_| ());

    result.map_err(|error| PublicError::from(crate::domain::error::AppError::Io(error)))
}
