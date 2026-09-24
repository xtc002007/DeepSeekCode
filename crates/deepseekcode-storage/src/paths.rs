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

/// Compare two directory paths after canonicalization.
/// Case-insensitive ONLY on Windows; Linux/macOS stay case-sensitive so
/// `/Tmp/A` and `/tmp/a` do not collide.
pub fn directory_paths_equal(a: &str, b: &str) -> bool {
    if a.trim().is_empty() && b.trim().is_empty() {
        return true;
    }
    let na = normalize_directory_path(a);
    let nb = normalize_directory_path(b);
    #[cfg(windows)]
    {
        na.eq_ignore_ascii_case(&nb)
    }
    #[cfg(not(windows))]
    {
        na == nb
    }
}

/// Normalized key for SQL lookups of `working_directory`.
/// Lowercases ONLY on Windows; preserves case on Unix. Also strips
/// `\\?\` / `\\.\` verbatim prefixes and trailing slashes uniformly so
/// Rust and SQL (`lower(rtrim(replace(...)))`) agree.
pub fn normalize_directory_path_for_lookup(path: &str) -> String {
    let normalized = normalize_directory_path(path)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    #[cfg(windows)]
    {
        normalized.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

fn strip_verbatim_prefix(path: std::path::PathBuf) -> String {
    let s = path.display().to_string();
    for prefix in [r"\\?\", r"\\.\"] {
        if let Some(stripped) = s.strip_prefix(prefix) {
            return stripped.to_string();
        }
    }
    s
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
