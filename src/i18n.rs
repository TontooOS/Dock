//! Translations via Accessibility (`lang/en_us.json`, `lang/de_de.json`).
//!
//! The same two locales every TontooOS app ships; the lookup order walks from
//! the packaged bundle over the build tree to the staged system location.

use std::path::PathBuf;

use crate::Accessibility::{LangFile, LangStore};

static LOCALE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// System language: `de_de` when the locale starts with `de`, else `en_us`.
pub fn sys_lang() -> String {
    for key in ["LANGUAGE", "LANG", "LC_ALL"] {
        if let Ok(value) = std::env::var(key) {
            if value.is_empty() {
                continue;
            }
            return locale_of(&value);
        }
    }
    if let Ok(content) = std::fs::read_to_string("/etc/locale.conf") {
        for line in content.lines() {
            if let Some(lang) = line.trim().strip_prefix("LANG=") {
                return locale_of(lang);
            }
        }
    }
    "en_us".to_string()
}

fn locale_of(value: &str) -> String {
    if value.to_lowercase().starts_with("de") {
        "de_de".to_string()
    } else {
        "en_us".to_string()
    }
}

fn lang_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(env) = std::env::var("DOCK_LANG_DIR") {
        if !env.is_empty() {
            dirs.push(PathBuf::from(env));
        }
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lang"));
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("lang"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("lang"));
            if let Some(grand) = parent.parent() {
                dirs.push(grand.join("lang"));
                dirs.push(grand.join("Resources").join("lang"));
            }
        }
    }
    dirs.push(PathBuf::from("/usr/share/tontoo/dock/lang"));
    dirs
}

/// Load the strings for the detected locale. Safe to call repeatedly.
pub fn init_i18n() {
    if LOCALE.get().is_some() {
        return;
    }
    let locale = sys_lang();
    let mut files: Vec<LangFile> = Vec::new();
    for dir in lang_dirs() {
        for code in ["en_us", "de_de"] {
            let path = dir.join(format!("{code}.json"));
            if let Ok(file) = LangFile::from_file(&path) {
                if !files.iter().any(|f| f.lang == file.lang) {
                    files.push(file);
                }
            }
        }
    }
    if !files.is_empty() {
        if let Err(err) = LangStore::init(files, Some("en_us".to_string())) {
            eprintln!("[dock] i18n init failed: {err}");
        }
    }
    let _ = LOCALE.set(locale);
}

/// Translate `key` with the system language, or `fallback` when the key is
/// missing (so a demo program without a lang entry still shows its name).
pub fn trk(key: &str, fallback: &str) -> String {
    init_i18n();
    let locale = LOCALE.get().cloned().unwrap_or_else(|| "en_us".to_string());
    let translated = LangStore::instance().t(&locale, key, None).unwrap_or_default();
    if translated.is_empty() {
        fallback.to_string()
    } else {
        translated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_files_load() {
        init_i18n();
        // Packaged files must resolve (cargo test runs at the crate root).
        assert_eq!(trk("dock.app.launchpad", "?"), "Launchpad");
    }

    #[test]
    fn missing_key_returns_fallback() {
        assert_eq!(trk("no.such.key", "fallback"), "fallback");
    }

    #[test]
    fn german_locale_is_detected() {
        assert_eq!(locale_of("de_DE.UTF-8"), "de_de");
        assert_eq!(locale_of("en_US.UTF-8"), "en_us");
        assert_eq!(locale_of("C"), "en_us");
    }
}