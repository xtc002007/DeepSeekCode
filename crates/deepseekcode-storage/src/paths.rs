//! Canonical directory paths for stable comparison across platforms.

/// Resolve a directory path to its canonical form for storage and comparison.
pub fn normalize_directory_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    let pb = std::path::Path::new(path);
    let canonical = std::fs::canonicalize(pb).unwrap_or_else(|_| pb.to_path_buf());
    strip_verbatim_prefix(canonical)
}

/// Compare two directory paths after canonicalization (case-insensitive on Windows).
pub fn directory_paths_equal(a: &str, b: &str) -> bool {
    if a.trim().is_empty() && b.trim().is_empty() {
        return true;
    }
    normalize_directory_path(a).eq_ignore_ascii_case(&normalize_directory_path(b))
}

/// Normalized key for SQL lookups of `working_directory`.
pub fn normalize_directory_path_for_lookup(path: &str) -> String {
    normalize_directory_path(path)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

fn strip_verbatim_prefix(path: std::path::PathBuf) -> String {
    let s = path.display().to_string();
    #[cfg(windows)]
    {
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
    }
    #[cfg(not(windows))]
    {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn canonical_paths_compare_equal() {
        let dir = TempDir::new().expect("tempdir");
        let canonical = normalize_directory_path(&dir.path().to_string_lossy());
        assert!(directory_paths_equal(
            &canonical,
            &dir.path().to_string_lossy()
        ));
    }

    #[test]
    fn lookup_key_matches_canonical_compare() {
        let dir = TempDir::new().expect("tempdir");
        let a = normalize_directory_path_for_lookup(&dir.path().to_string_lossy());
        let b = normalize_directory_path_for_lookup(&normalize_directory_path(
            &dir.path().to_string_lossy(),
        ));
        assert_eq!(a, b);
    }
}
