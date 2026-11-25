//! Build script. Besides Tauri's own step, it bakes the Google OAuth client (for Google Drive backup) into the app.
//!
//! The values come from the environment (GitHub Actions secrets in official releases) or, for local builds, from a
//! `.env` file next to package.json (see .env.example). They are never printed. Without them the app is built
//! without Google Drive and says so in Sync & backup.

use std::path::Path;

const KEYS: [&str; 2] = ["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"];

fn from_dotenv(key: &str) -> Option<String> {
    for file in ["../.env", ".env"] {
        let Ok(text) = std::fs::read_to_string(Path::new(file)) else { continue };
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !v.is_empty() {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=../.env");
    println!("cargo:rerun-if-changed=.env");
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={key}");
        let value = std::env::var(key).ok().filter(|v| !v.trim().is_empty()).or_else(|| from_dotenv(key));
        if let Some(v) = value {
            // Only letters, digits and the punctuation real client values use, so nothing odd reaches the compiler.
            if v.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
                println!("cargo:rustc-env=CHATVAULT_{key}={v}");
            } else {
                println!("cargo:warning={key} has unexpected characters and was ignored");
            }
        }
    }
    tauri_build::build()
}
