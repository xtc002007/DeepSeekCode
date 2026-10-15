//! WASM plugin trust: minisign sidecars + hash pinning.
//!
//! Threat model: `.wasm` files execute with tool-execution proxy access, so
//! a swapped binary is a full session compromise. Previously loading only
//! checked "instantiates OK". Trust tiers, strongest first:
//!
//! 1. **minisign sidecar** (`<plugin>.wasm.minisig`): verified with an
//!    external `minisig` binary against `PluginTrust::trusted_keys`. Bad
//!    signature = hard error, always.
//! 2. **Hash pinning** (`.code-plus-plus/plugins.lock.json`): first load records
//!    the sha256 (warn loudly); later loads must match (mismatch = hard
//!    error). Covers unsigned plugins without trusting them blindly.
//! 3. **Unsigned, unpinned**: allowed only when `require_signatures` is false
//!    (default), with a warning directing operators to sign or pin.
//!
//! With `require_signatures = true`, tiers 2–3 refuse to load.

use code_plus_plus_types::{CodePlusPlusError, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Trust policy for WASM plugin loads. Built from
/// `plugins_require_signatures` / `plugins_trusted_keys` config.
#[derive(Debug, Clone, Default)]
pub struct PluginTrust {
    pub require_signatures: bool,
    pub trusted_keys: Vec<String>,
}

/// Outcome of a trust check (for logging/telemetry; load proceeds on Ok).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustOutcome {
    MinisignVerified,
    HashPinned,
    HashRecorded,
    UnsignedAllowed,
}

/// Verify `wasm_path` under `trust`, recording pins in `pins_dir`
/// (the project's `.code-plus-plus/` directory).
pub fn verify_wasm_file(
    wasm_path: &Path,
    pins_dir: &Path,
    trust: &PluginTrust,
) -> Result<TrustOutcome> {
    let bytes = std::fs::read(wasm_path).map_err(|e| {
        CodePlusPlusError::Other(format!(
            "cannot read wasm plugin {}: {e}",
            wasm_path.display()
        ))
    })?;
    let digest = hex_of(&bytes);

    // Tier 1: minisign sidecar.
    let sig_path = wasm_path.with_extension("wasm.minisig");
    if sig_path.is_file() {
        // Without a `minisign` binary there is nothing to verify with: fall
        // through to hash pinning (non-strict) instead of erroring, but say so.
        let have_minisign = std::process::Command::new("minisign")
            .arg("-v")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if have_minisign {
            return verify_minisig_sidecar(wasm_path, &sig_path, trust);
        }
        if trust.require_signatures {
            return Err(CodePlusPlusError::Other(format!(
                "cannot verify {} signature: minisign binary not found",
                wasm_path.display()
            )));
        }
        tracing::warn!(
            target: code_plus_plus_telemetry::APP,
            path = %wasm_path.display(),
            "minisign binary not found; signed plugin falls back to hash pinning"
        );
    }

    // Tier 3 gate before pinning: strict mode refuses unsigned code.
    if trust.require_signatures {
        return Err(CodePlusPlusError::Other(format!(
            "wasm plugin {} has no .minisig sidecar and plugins_require_signatures is set",
            wasm_path.display()
        )));
    }

    // Tier 2: hash pinning.
    let pins_path = pins_dir.join("plugins.lock.json");
    let mut pins = read_pins(&pins_path);
    let file_name = wasm_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match pins.get(&file_name) {
        Some(pinned) if pinned == &digest => Ok(TrustOutcome::HashPinned),
        Some(pinned) => Err(CodePlusPlusError::Other(format!(
            "wasm plugin {} hash mismatch: pinned {pinned}, actual {digest}; refusing to load (delete the pin to re-trust explicitly)",
            wasm_path.display()
        ))),
        None => {
            pins.insert(file_name, digest);
            if let Err(e) = write_pins(&pins_path, &pins) {
                tracing::warn!(
                    target: code_plus_plus_telemetry::APP,
                    path = %pins_path.display(),
                    error = %e,
                    "could not persist wasm plugin pin; continuing without a pin"
                );
            }
            tracing::warn!(
                target: code_plus_plus_telemetry::APP,
                path = %wasm_path.display(),
                "unsigned wasm plugin recorded in plugins.lock.json; sign it or enable plugins_require_signatures"
            );
            Ok(TrustOutcome::HashRecorded)
        }
    }
}

fn hex_of(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_pins(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_pins(path: &Path, pins: &HashMap<String, String>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Atomic: tmp + rename so a crashed write never leaves half JSON.
    let tmp = path.with_extension("lock.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(pins).unwrap_or_default())?;
    std::fs::rename(&tmp, path)
}

/// Verify `<wasm>.minisig` with an external `minisign` binary against the
/// configured trusted keys. Missing binary degrades to hash pinning with a
/// warning (unless strict mode, which errors).
fn verify_minisig_sidecar(
    wasm_path: &Path,
    sig_path: &Path,
    trust: &PluginTrust,
) -> Result<TrustOutcome> {
    if trust.trusted_keys.is_empty() {
        if trust.require_signatures {
            return Err(CodePlusPlusError::Other(format!(
                "wasm plugin {} is signed but no plugins_trusted_keys are configured",
                wasm_path.display()
            )));
        }
        tracing::warn!(
            target: code_plus_plus_telemetry::APP,
            path = %wasm_path.display(),
            "wasm plugin has a .minisig sidecar but no trusted keys are configured; skipping signature check"
        );
        return Err(CodePlusPlusError::Other(format!(
            "wasm plugin {} signature cannot be verified without trusted keys",
            wasm_path.display()
        )));
    }
    // minisign pubkey format: "untrusted comment: ...\n<base64 keynum+key>".
    // Try each trusted key in its own temp file; any success verifies.
    let mut last_err = String::new();
    for (i, key) in trust.trusted_keys.iter().enumerate() {
        let key_path = std::env::temp_dir().join(format!(
            "code-plus-plus-pluginkey-{i}-{}.pub",
            std::process::id()
        ));
        if std::fs::write(&key_path, key.trim_end_matches('\n').to_string() + "\n").is_err() {
            continue;
        }
        // Restrict temp key readability like the signing-key path.
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("icacls")
                .args([
                    key_path.to_string_lossy().as_ref(),
                    "/inheritance:r",
                    "/grant:r",
                    &format!("{}:(R)", std::env::var("USERNAME").unwrap_or_default()),
                ])
                .output();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
        let status = std::process::Command::new("minisign")
            .args([
                "-Vm",
                wasm_path.to_string_lossy().as_ref(),
                "-p",
                key_path.to_string_lossy().as_ref(),
                "-x",
                sig_path.to_string_lossy().as_ref(),
            ])
            .output();
        let _ = std::fs::remove_file(&key_path);
        match status {
            Ok(out) if out.status.success() => {
                tracing::info!(
                    target: code_plus_plus_telemetry::APP,
                    path = %wasm_path.display(),
                    key_index = i,
                    "wasm plugin minisign signature verified"
                );
                return Ok(TrustOutcome::MinisignVerified);
            }
            Ok(out) => {
                last_err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No minisign binary: degrade deliberately, not silently.
                if trust.require_signatures {
                    return Err(CodePlusPlusError::Other(format!(
                        "cannot verify {} signature: minisign execution failed",
                        wasm_path.display()
                    )));
                }
                tracing::warn!(
                    target: code_plus_plus_telemetry::APP,
                    path = %wasm_path.display(),
                    "minisign spawn failed despite pre-check; treating as verification failure"
                );
                last_err = format!("minisign execution failed for {}", wasm_path.display());
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }
    Err(CodePlusPlusError::Other(format!(
        "wasm plugin {} signature invalid: {last_err}",
        wasm_path.display()
    )))
}

/// Resolve the pins directory for a wasm plugin path being loaded from an
/// explicit config entry: `<project>/.code-plus-plus`. Falls back to the
/// plugin's own parent when the layout is unknown.
pub fn pins_dir_for(project_dir: &str, wasm_path: &Path) -> PathBuf {
    let candidate = Path::new(project_dir).join(".code-plus-plus");
    if candidate.is_dir()
        || !wasm_path
            .parent()
            .is_some_and(|p| p.join("plugins.lock.json").is_file())
    {
        return candidate;
    }
    wasm_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| candidate)
}
