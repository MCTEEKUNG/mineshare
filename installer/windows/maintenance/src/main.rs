//! MSI helper. User data is never an installer-owned component.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::process::{Command, ExitCode};

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 || !["prepare", "complete", "rollback"].contains(&args[0].as_str()) {
        eprintln!("Expected action, installation directory, transaction GUID and version");
        return ExitCode::FAILURE;
    }
    // No user-controlled values are interpolated into PowerShell source.
    let bytes: Vec<_> = include_str!("../../maintenance.ps1")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &base64(&bytes),
        ])
        .env("MINESHARE_INSTALL_ACTION", &args[0])
        .env("MINESHARE_INSTALL_DIRECTORY", &args[1])
        .env("MINESHARE_INSTALL_TRANSACTION", &args[2])
        .env("MINESHARE_INSTALL_VERSION", &args[3]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        other => {
            eprintln!("Installer maintenance failed: {other:?}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn powershell_encoding_padding() {
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(&[]), "");
    }
}
