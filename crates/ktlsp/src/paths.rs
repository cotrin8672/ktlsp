use std::path::{Path, PathBuf};

/// Preserve an explicit Unix-style home while supporting native Windows launches.
pub(crate) fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(std::env::home_dir)
}

/// Match editor file-URI keys without resolving the filesystem or changing Unix filenames.
pub fn file_key(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            format!("//{unc}")
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(&path).to_string()
        };
        let mut key = path.replace('\\', "/");
        if key.as_bytes().get(1) == Some(&b':') {
            key[..1].make_ascii_uppercase();
        }
        key
    }
    #[cfg(not(windows))]
    path.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn file_key_matches_native_verbatim_and_uri_paths() {
        for path in [
            r"c:\project\Main.kt",
            r"\\?\C:\project\Main.kt",
            "C:/project/Main.kt",
        ] {
            assert_eq!(file_key(Path::new(path)), "C:/project/Main.kt");
        }
        for path in [r"\\server\share\Main.kt", r"\\?\UNC\server\share\Main.kt"] {
            assert_eq!(file_key(Path::new(path)), "//server/share/Main.kt");
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn file_key_preserves_unix_backslashes() {
        assert_eq!(file_key(Path::new(r"/project/a\b.kt")), r"/project/a\b.kt");
    }
}
