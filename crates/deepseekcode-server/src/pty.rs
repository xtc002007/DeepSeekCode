use deepseekcode_types::{DeepSeekCodeError, PtyId, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub const PTY_TICKET_HEADER: &str = "x-deepseekcode-ticket";
pub const PTY_TICKET_HEADER_VALUE: &str = "1";
pub const PTY_TICKET_QUERY: &str = "ticket";
pub const PTY_TICKET_TTL: Duration = Duration::from_secs(60);

pub struct PtySession {
    pub id: PtyId,
    pub directory: String,
    pub writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Handle to the spawned shell so `destroy` can terminate it outright
    /// instead of relying on PTY-master teardown.
    pub child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    pub input_tx: broadcast::Sender<Vec<u8>>,
}

struct PtyTicketRecord {
    pty_id: String,
    directory: String,
    expires_at: Instant,
}

pub struct PtyManager {
    sessions: Mutex<HashMap<String, Arc<PtySession>>>,
    tickets: Mutex<HashMap<String, PtyTicketRecord>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            tickets: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(&self, cwd: &str, rows: u16, cols: u16) -> Result<Arc<PtySession>> {
        let cwd = normalize_pty_cwd(cwd);
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;

        let shell = if cfg!(windows) {
            std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".into())
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
        };

        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(&cwd);
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;

        let master = Arc::new(Mutex::new(pair.master));
        let mut reader = master
            .lock()
            .unwrap()
            .try_clone_reader()
            .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;
        let writer = master
            .lock()
            .unwrap()
            .take_writer()
            .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;

        let id = PtyId::new();
        tracing::info!(
            target: deepseekcode_telemetry::SERVER,
            pty_id = %id,
            cwd = cwd,
            rows = rows,
            cols = cols,
            "pty created"
        );
        let (input_tx, _) = broadcast::channel(256);
        let input_tx_clone = input_tx.clone();
        let writer = Arc::new(Mutex::new(writer));

        let pty_id_log = id.to_string();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        tracing::info!(
                            target: deepseekcode_telemetry::SERVER,
                            pty_id = %pty_id_log,
                            "pty disconnected"
                        );
                        break;
                    }
                    Ok(n) => {
                        let _ = input_tx_clone.send(buf[..n].to_vec());
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: deepseekcode_telemetry::SERVER,
                            pty_id = %pty_id_log,
                            error = %e,
                            "pty read error"
                        );
                        break;
                    }
                }
            }
        });

        let session = Arc::new(PtySession {
            id,
            directory: cwd.to_string(),
            writer,
            master,
            child: Arc::new(Mutex::new(child)),
            input_tx,
        });
        self.sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), session.clone());
        Ok(session)
    }

    pub fn issue_ticket(&self, pty_id: &str, directory: &str) -> Result<(String, u64)> {
        if self.get(pty_id).is_none() {
            return Err(DeepSeekCodeError::NotFound(format!("pty {pty_id}")));
        }
        let ticket = ulid::Ulid::new().to_string();
        let expires_at = Instant::now() + PTY_TICKET_TTL;
        self.tickets.lock().unwrap().insert(
            ticket.clone(),
            PtyTicketRecord {
                pty_id: pty_id.to_string(),
                directory: directory.to_string(),
                expires_at,
            },
        );
        Ok((ticket, PTY_TICKET_TTL.as_secs()))
    }

    pub fn consume_ticket(&self, ticket: &str, pty_id: &str, directory: &str) -> bool {
        let mut tickets = self.tickets.lock().unwrap();
        tickets.retain(|_, record| record.expires_at > Instant::now());
        let Some(record) = tickets.remove(ticket) else {
            return false;
        };
        record.pty_id == pty_id && record.directory == directory
    }

    pub fn get(&self, id: &str) -> Option<Arc<PtySession>> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<()> {
        let session = self
            .get(id)
            .ok_or_else(|| DeepSeekCodeError::NotFound(format!("pty {id}")))?;
        session
            .writer
            .lock()
            .unwrap()
            .write_all(data)
            .map_err(DeepSeekCodeError::Io)?;
        Ok(())
    }

    /// Remove a PTY session: kill the child shell, drop any live tickets.
    /// `directory` must match the directory the PTY was created under, so one
    /// project cannot destroy another project's terminals.
    pub fn destroy(&self, id: &str, directory: &str) -> Result<()> {
        let requested = normalize_pty_cwd(directory);
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            let session = sessions
                .get(id)
                .cloned()
                .ok_or_else(|| DeepSeekCodeError::NotFound(format!("pty {id}")))?;
            if session.directory != requested {
                return Err(DeepSeekCodeError::NotFound(format!(
                    "pty {id} does not belong to this project directory"
                )));
            }
            sessions.remove(id);
            session
        };
        if let Ok(mut child) = session.child.lock() {
            if let Err(e) = child.kill() {
                tracing::warn!(
                    target: deepseekcode_telemetry::SERVER,
                    pty_id = %id,
                    error = %e,
                    "pty child kill failed"
                );
            }
        }
        self.tickets
            .lock()
            .unwrap()
            .retain(|_, record| record.pty_id != id);
        Ok(())
    }

    pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<()> {
        let session = self
            .get(id)
            .ok_or_else(|| DeepSeekCodeError::NotFound(format!("pty {id}")))?;
        session
            .master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| DeepSeekCodeError::Other(e.to_string()))?;
        Ok(())
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_pty_cwd(cwd: &str) -> String {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
    }
    let pb = std::path::Path::new(cwd);
    let canonical = std::fs::canonicalize(pb).unwrap_or_else(|_| pb.to_path_buf());
    let s = canonical.display().to_string();
    #[cfg(windows)]
    {
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
    }
    #[cfg(not(windows))]
    {
        s
    }
}
