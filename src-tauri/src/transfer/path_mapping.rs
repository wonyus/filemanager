use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::domain::error::AppError;

const MAX_SEGMENT_BYTES: usize = 255;

/// Maps an object key relative to a selected prefix into a safe local path.
/// The mapping is lexical (it does not require the destination to exist) and
/// therefore works for a brand-new download directory as well as an existing
/// one. Every key segment is encoded deterministically and cannot escape the
/// destination through `..`, absolute paths, or Windows device names.
pub fn map_key_to_local(
    destination_root: &Path,
    selected_prefix: &str,
    object_key: &str,
) -> Result<PathBuf, AppError> {
    let relative = relative_key(selected_prefix, object_key)?;
    let mut output = destination_root.to_path_buf();
    let mut relative_components = Vec::new();
    for segment in relative.split('/') {
        let encoded = encode_segment(segment)?;
        relative_components.push(encoded);
    }
    for component in &relative_components {
        output.push(component);
    }
    if output == destination_root {
        return Err(AppError::Validation(
            "object key has no downloadable name".to_string(),
        ));
    }
    Ok(output)
}

/// Add the deterministic `_s3c_<8hex>` suffix required when two remote keys
/// collapse to the same case-insensitive Windows path.  The hash is derived
/// from the complete remote key, so planned names remain stable across runs.
pub fn collision_path(path: &Path, remote_key: &str) -> PathBuf {
    let digest = Sha256::digest(remote_key.as_bytes());
    let suffix = digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let replacement = match path.extension().and_then(|value| value.to_str()) {
        Some(extension) if !extension.is_empty() => {
            let stem = name.strip_suffix(&format!(".{extension}")).unwrap_or(name);
            format!("{stem}_s3c_{suffix}.{extension}")
        }
        _ => format!("{name}_s3c_{suffix}"),
    };
    path.with_file_name(replacement)
}

fn relative_key(selected_prefix: &str, object_key: &str) -> Result<String, AppError> {
    let prefix = selected_prefix.trim_matches('/');
    if object_key.contains('\0') {
        return Err(AppError::Validation(
            "object key contains a NUL byte".to_string(),
        ));
    }
    let relative = if prefix.is_empty() {
        object_key
    } else if object_key == prefix {
        ""
    } else if let Some(rest) = object_key.strip_prefix(&format!("{prefix}/")) {
        rest
    } else {
        return Err(AppError::Validation(
            "object key is outside the selected prefix".to_string(),
        ));
    };
    Ok(relative.to_string())
}

fn encode_segment(segment: &str) -> Result<String, AppError> {
    let bytes = segment.as_bytes();
    let mut output = String::with_capacity(segment.len());
    if segment.is_empty() {
        // Empty S3 segments (for example `a//b`) are preserved as a safe
        // reversible marker instead of being silently dropped.
        return Ok("_s3x2F_".to_string());
    }
    let reserved_segment = segment == "." || segment == "..";
    for (index, character) in segment.char_indices() {
        let byte = bytes[index];
        if byte >= 0x80 {
            output.push(character);
            continue;
        }
        let trailing_dot_or_space =
            index + character.len_utf8() == bytes.len() && (byte == b'.' || byte == b' ');
        let unsafe_windows = matches!(
            byte,
            b'<' | b'>' | b':' | b'"' | b'/' | b'\\' | b'|' | b'?' | b'*'
        );
        let looks_like_escape = byte == b'_' && looks_like_escape(bytes, index);
        if byte.is_ascii_control()
            || unsafe_windows
            || trailing_dot_or_space
            || reserved_segment
            || looks_like_escape
        {
            output.push_str(&format!("_s3x{byte:02X}_"));
        } else {
            output.push(byte as char);
        }
    }
    if is_reserved_device_name(segment) {
        output.insert_str(0, "_s3r_");
    }
    if output.len() > MAX_SEGMENT_BYTES {
        return Err(AppError::Validation(
            "local path segment exceeds 255 bytes".to_string(),
        ));
    }
    Ok(output)
}

fn looks_like_escape(bytes: &[u8], index: usize) -> bool {
    if index + 6 > bytes.len() || bytes[index] != b'_' || bytes[index + 1] != b's' {
        return false;
    }
    bytes[index + 2] == b'3'
        && bytes[index + 3] == b'x'
        && bytes[index + 6] == b'_'
        && bytes[index + 4].is_ascii_hexdigit()
        && bytes[index + 5].is_ascii_hexdigit()
}

fn is_reserved_device_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or_default();
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_nested_key_under_prefix() {
        let path =
            map_key_to_local(Path::new("C:/downloads"), "photos/", "photos/2026/a b.jpg").unwrap();
        assert_eq!(path, PathBuf::from("C:/downloads/2026/a b.jpg"));
    }

    #[test]
    fn rejects_traversal_and_sibling_prefixes() {
        let traversal =
            map_key_to_local(Path::new("C:/downloads"), "photos/", "photos/../secret.txt").unwrap();
        assert!(traversal.to_string_lossy().contains("_s3x2E__s3x2E_"));
        assert!(
            map_key_to_local(Path::new("C:/downloads"), "photos/", "photos-old/a.txt").is_err()
        );
        let leading = map_key_to_local(Path::new("C:/downloads"), "", "../secret.txt").unwrap();
        assert!(leading.to_string_lossy().contains("_s3x2E__s3x2E_"));
    }

    #[test]
    fn encodes_windows_unsafe_and_reserved_names() {
        let path = map_key_to_local(Path::new("C:/downloads"), "", "CON.txt").unwrap();
        assert!(path.to_string_lossy().contains("_s3r_CON.txt"));
        let path = map_key_to_local(Path::new("C:/downloads"), "", "a:b.txt").unwrap();
        assert!(path.to_string_lossy().contains("a_s3x3A_b.txt"));
        let path = map_key_to_local(Path::new("C:/downloads"), "", "x_s3x3A_y").unwrap();
        assert!(path.to_string_lossy().contains("x_s3x5F_s3x3A_y"));
    }
}
