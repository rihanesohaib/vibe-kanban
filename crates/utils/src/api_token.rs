//! Per-launch API token shared between the server and local clients.
//!
//! The server generates a fresh token on every start and writes it to
//! `api-token` in the asset directory. Local clients (the MCP server, scripts)
//! read it from there, or from `VK_API_TOKEN` when set.

use std::{
    fmt::Write as _,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use rand::{RngCore, rngs::OsRng};

use crate::assets::asset_dir;

pub const API_TOKEN_FILE_NAME: &str = "api-token";
pub const API_TOKEN_ENV: &str = "VK_API_TOKEN";

const TOKEN_BYTES: usize = 32;

pub fn api_token_path() -> PathBuf {
    asset_dir().join(API_TOKEN_FILE_NAME)
}

/// 32 bytes from the OS CSPRNG, hex encoded.
pub fn generate_api_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let mut token = String::with_capacity(TOKEN_BYTES * 2);
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    token
}

/// Atomically replace the token file. The file is created with mode 0600 and
/// its directory is restricted to 0700 on Unix.
pub fn write_api_token_file(path: &Path, token: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "token path has no parent"))?;
    fs::create_dir_all(dir)?;
    restrict_permissions(dir, 0o700)?;

    let tmp = dir.join(format!(".{API_TOKEN_FILE_NAME}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&tmp);

    let result = (|| {
        let mut file = create_private_file(&tmp)?;
        restrict_permissions(&tmp, 0o600)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Token from `VK_API_TOKEN`, falling back to the token file.
pub fn read_api_token() -> Option<String> {
    if let Ok(value) = std::env::var(API_TOKEN_ENV) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    read_api_token_file(&api_token_path())
}

pub fn read_api_token_file(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let token = contents.trim();
    (!token.is_empty()).then(|| token.to_string())
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_64_hex_chars_and_unique() {
        let a = generate_api_token();
        let b = generate_api_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn write_then_read_round_trips_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join(API_TOKEN_FILE_NAME);

        write_api_token_file(&path, "first").unwrap();
        assert_eq!(read_api_token_file(&path).as_deref(), Some("first"));

        write_api_token_file(&path, "second").unwrap();
        assert_eq!(read_api_token_file(&path).as_deref(), Some("second"));

        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| name != API_TOKEN_FILE_NAME)
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn missing_or_empty_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(API_TOKEN_FILE_NAME);
        assert_eq!(read_api_token_file(&path), None);
        fs::write(&path, "  \n").unwrap();
        assert_eq!(read_api_token_file(&path), None);
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_0600_and_directory_is_0700() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        fs::create_dir(&data_dir).unwrap();
        fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let path = data_dir.join(API_TOKEN_FILE_NAME);

        write_api_token_file(&path, "secret").unwrap();

        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(&data_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }
}
