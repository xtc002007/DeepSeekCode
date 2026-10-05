//! Agent-visible PTY execution for shell tools.
//!
//! Product contract: every agent-spawned command must run in a REAL,
//! interactive PTY (same input/output behavior as a hand-opened shell),
//! listed while running and destroyed on completion. Re-running the command
//! a second time into a display shell would execute side effects twice, so
//! the PTY here is the ONE AND ONLY execution site: the tool spawns
//! `shell -c command` (argv, never textual wrapping) attached to the slave,
//! captures the merged stream + exit status for the LLM result, and the
//! embedded server proxies the same master to desktop WebSockets.
//!
//! Deliberate divergences from the pipe backend (which stays the default):
//! - stdout/stderr are MERGED by the terminal by design; the result carries
//!   the merged text in `stdout` and `""` in `stderr`.
//! - ANSI escapes are stripped for the result; the live display keeps them.
//! - ConPTY emits UTF-8, so no OEM codepage dance is needed here.
//!
//! Gating: `BashTool` runs pipes unless constructed `.with_pty(true)`; the
//! registry enables it from `DEEPSEEKCODE_BASH_PTY=1` (set by the desktop at
//! startup). CLI / server / tests keep pipes unless they opt in, so every
//! existing stdout/stderr-separation test keeps passing unchanged.

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::broadcast;

/// Matches the desktop grid so program table layout never wraps client-side.
pub const AGENT_PTY_COLS: u16 = 120;
pub const AGENT_PTY_ROWS: u16 = 200;
const BROADCAST_CAP: usize = 512;

/// Env gate read by the tool registry (never per-call: process env is global
/// and tests must stay race-free; tests opt in via `BashTool::with_pty`).
pub fn agent_pty_enabled() -> bool {
    matches!(
        std::env::var("DEEPSEEKCODE_BASH_PTY")
            .unwrap_or_default()
            .as_str(),
        "1" | "true" | "TRUE" | "yes" | "YES"
    )
}

pub struct AgentPtySession {
    pub id: String,
    pub directory: String,
    pub writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub input_tx: broadcast::Sender<Vec<u8>>,
    /// Kept alive: dropping the last master handle can tear down the PTY
    /// out from under the running child on some platforms. `Mutex`-wrapped
    /// (never locked) solely so the session stays `Sync` for the
    /// process-global registry.
    _master: Mutex<Box<dyn MasterPty + Send>>,
}

pub struct AgentPtyRegistry {
    sessions: Mutex<HashMap<String, Arc<AgentPtySession>>>,
}

impl AgentPtyRegistry {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Spawn `program`+`args` under a fresh PTY in `cwd`. Returns the
    /// registered session, the child handle (owned by the tool future for
    /// wait/kill), and a broadcast receiver for the runner loop.
    pub fn spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        envs: &[(String, String)],
    ) -> Result<
        (
            Arc<AgentPtySession>,
            Box<dyn portable_pty::Child + Send + Sync>,
            broadcast::Receiver<Vec<u8>>,
        ),
        String,
    > {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: AGENT_PTY_ROWS,
                cols: AGENT_PTY_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        // `normalize_pty_cwd`-style trimming only: never canonicalize here,
        // the desktop matches ownership after its own canonicalization.
        cmd.cwd(cwd.trim());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| e.to_string())?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| e.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| e.to_string())?;

        let id = ulid::Ulid::new().to_string();
        let (input_tx, _) = broadcast::channel(BROADCAST_CAP);
        let session = Arc::new(AgentPtySession {
            id: id.clone(),
            directory: cwd.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            input_tx: input_tx.clone(),
            _master: Mutex::new(pair.master),
        });
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, session.clone());

        let reader_tx = input_tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if reader_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok((session, child, input_tx.subscribe()))
    }

    pub fn get(&self, id: &str) -> Option<Arc<AgentPtySession>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }

    pub fn write(&self, id: &str, data: &[u8]) -> bool {
        let Some(session) = self.get(id) else {
            return false;
        };
        // Bind before returning: the guard borrows `session`, so it must
        // drop (statement end) before `session` itself does (block end).
        let wrote = session
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write_all(data)
            .is_ok();
        wrote
    }

    pub fn remove(&self, id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id);
    }
}

impl Default for AgentPtyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_AGENT_PTY_REGISTRY: LazyLock<AgentPtyRegistry> =
    LazyLock::new(AgentPtyRegistry::new);

/// Process-global agent PTY table, shared by the tool runner (spawns) and
/// the embedded server (WebSocket attach). Same-process by construction:
/// the desktop always drives an embedded engine.
pub fn global_agent_pty_registry() -> &'static AgentPtyRegistry {
    &GLOBAL_AGENT_PTY_REGISTRY
}

/// Strip ANSI escape sequences for the LLM-facing result.
///
/// Handles CSI (`ESC [ … final-byte`), OSC (`ESC ] … BEL` / `ESC \`), and
/// lone `ESC` + single char. The live display keeps raw bytes (colors are a
/// feature there); the stored result must stay plain text like the pipe
/// backend produces.
pub fn strip_ansi_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI: parameter bytes 0x30–0x3F, intermediates 0x20–0x2F,
                // final byte 0x40–0x7E.
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                // OSC: terminated by BEL or ESC \.
                let mut prev_esc = false;
                for c in chars.by_ref() {
                    if c == '\x07' || (prev_esc && c == '\\') {
                        break;
                    }
                    prev_esc = c == '\x1b';
                }
            }
            Some(_) => {
                // Single-char escape (e.g. `ESC =`, `ESC >`, `ESC M`).
                chars.next();
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strip_ansi_escapes;

    #[test]
    fn strips_sgr_colors_but_keeps_text() {
        assert_eq!(
            strip_ansi_escapes("\x1b[31merror\x1b[0m plain"),
            "error plain"
        );
    }

    #[test]
    fn strips_cursor_and_erase_sequences() {
        assert_eq!(
            strip_ansi_escapes("\x1b[?25lhi\x1b[2K\x1b[1Gbye"),
            "hibye"
        );
    }

    #[test]
    fn strips_osc_title_and_lone_escapes() {
        assert_eq!(
            strip_ansi_escapes("\x1b]0;title\x07ok\x1bM"),
            "ok"
        );
    }

    #[test]
    fn plain_text_is_untouched() {
        let text = "D:\\Workspace\\Echoo> dir\n.bin\ntypescript";
        assert_eq!(strip_ansi_escapes(text), text);
    }

    #[test]
    fn spawn_and_write_round_trip() {
        // Registry mechanics only (no shell semantics asserted): spawn a
        // short-lived sleeper, write stdin bytes, remove the entry.
        let registry = super::AgentPtyRegistry::new();
        let (program, args): (String, Vec<String>) = if cfg!(windows) {
            ("cmd".into(), vec!["/C".into(), "echo probe-echo".into()])
        } else {
            ("/bin/sh".into(), vec!["-c".into(), "echo probe-echo".into()])
        };
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        let (session, mut child, rx) = registry
            .spawn(&program, &args, &cwd, &[])
            .expect("pty spawn works");
        assert!(registry.get(&session.id).is_some());
        assert!(registry.write(&session.id, b""));
        drop(rx);
        let _ = child.wait();
        registry.remove(&session.id);
        assert!(registry.get(&session.id).is_none());
    }
}
