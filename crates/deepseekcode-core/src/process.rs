//! Cross-platform subprocess helpers (Windows: suppress console windows).

use deepseekcode_types::SessionId;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

static SESSION_CHILD_PIDS: OnceLock<Mutex<HashMap<SessionId, Vec<u32>>>> = OnceLock::new();

fn session_child_pids() -> &'static Mutex<HashMap<SessionId, Vec<u32>>> {
    SESSION_CHILD_PIDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Track a spawned subprocess PID so it can be killed when the session is interrupted.
pub fn register_session_child_pid(session_id: SessionId, pid: u32) {
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        session_id = %session_id,
        pid = pid,
        "session child pid registered"
    );
    session_child_pids()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(session_id)
        .or_default()
        .push(pid);
}

/// Remove a finished subprocess PID from the session registry.
pub fn unregister_session_child_pid(session_id: SessionId, pid: u32) {
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        session_id = %session_id,
        pid = pid,
        "session child pid unregistered"
    );
    let mut guard = session_child_pids()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(pids) = guard.get_mut(&session_id) {
        pids.retain(|existing| *existing != pid);
        if pids.is_empty() {
            guard.remove(&session_id);
        }
    }
}

/// RAII registration of a spawned subprocess PID. Unregisters on drop so every
/// exit path of a tool (normal completion, timeout, cancelled future) leaves
/// the registry consistent — a stale PID could otherwise be reaped against an
/// unrelated recycled process later.
pub struct SessionChildPidGuard {
    session_id: SessionId,
    pid: u32,
    armed: bool,
}

impl SessionChildPidGuard {
    pub fn register(session_id: SessionId, pid: u32) -> Self {
        register_session_child_pid(session_id, pid);
        Self {
            session_id,
            pid,
            armed: true,
        }
    }

    /// Detach without unregistering (used when ownership is handed over).
    /// Consumes self and returns an explicit [`DetachedPid`] that MUST be
    /// consumed (see `#[must_use]`): the PID stays tracked until the holder
    /// calls [`DetachedPid::unregister`] or [`DetachedPid::release`].
    /// Explicit handle (instead of `mem::forget`) makes exception-path leaks
    /// visible at the type level instead of silently leaking the guard.
    pub fn disarm(self) -> DetachedPid {
        let detached = DetachedPid {
            session_id: self.session_id,
            pid: self.pid,
            consumed: false,
        };
        // Suppress `Drop::unregister` without `mem::forget`: wrap in
        // `ManuallyDrop` so the guard shell (no heap allocation) is simply
        // not dropped. The PID stays registered under `detached`.
        let mut this = std::mem::ManuallyDrop::new(self);
        this.armed = false;
        detached
    }
}

impl Drop for SessionChildPidGuard {
    fn drop(&mut self) {
        if self.armed {
            unregister_session_child_pid(self.session_id, self.pid);
        }
    }
}

/// Explicit ownership of a still-tracked child PID after
/// [`SessionChildPidGuard::disarm`]. `#[must_use]` forces callers to handle
/// it: either [`DetachedPid::unregister`] (normal completion) or
/// [`DetachedPid::release`] (transfer to an external reaper). Dropping
/// without consuming logs a warning so exception-path leaks are visible
/// instead of silent.
#[must_use = "DetachedPid must be explicitly released or unregistered, otherwise the PID stays tracked"]
pub struct DetachedPid {
    session_id: SessionId,
    pid: u32,
    consumed: bool,
}

impl DetachedPid {
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Keep the PID tracked and hand ownership to an external reaper.
    /// Consumes self without unregistering (explicit, auditable).
    pub fn release(mut self) {
        self.consumed = true;
        // `self` drops here with `consumed=true`, so `Drop` below skips the
        // leak warning. The PID intentionally stays registered.
    }

    /// Normal completion: remove the PID from the registry.
    pub fn unregister(mut self) {
        self.consumed = true;
        unregister_session_child_pid(self.session_id, self.pid);
    }
}

impl Drop for DetachedPid {
    fn drop(&mut self) {
        if !self.consumed {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                session_id = %self.session_id,
                pid = self.pid,
                "DetachedPid dropped without release/unregister; PID stays tracked (exception-path leak)"
            );
        }
    }
}

/// Kill all tracked subprocesses for a session.
pub async fn kill_session_children(session_id: SessionId) {
    tracing::info!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        session_id = %session_id,
        "kill_session_children started"
    );
    let pids = session_child_pids()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&session_id)
        .unwrap_or_default();
    if pids.is_empty() {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            session_id = %session_id,
            "kill_session_children: no children"
        );
        return;
    }
    let count = pids.len();
    let _ = tokio::task::spawn_blocking(move || {
        for pid in pids {
            kill_pid(pid);
        }
    })
    .await;
    tracing::info!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        session_id = %session_id,
        count = count,
        "kill_session_children completed"
    );
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        // The child was started with setpgid(0, 0) (see configure_process_group),
        // so killing the process group takes down the whole tree — `bash -lc`
        // grandchildren included. Fall back to the direct PID if the group is
        // already gone.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/F", "/T"]);
        hide_console(&mut cmd);
        let _ = cmd.status();
    }
}

/// Kill a child and its whole descendant tree, given only its PID.
///
/// Needed by the bash **timeout** path, which historically relied on
/// `kill_on_drop` alone. That kills only the direct child (`cmd.exe` /
/// `bash`), so `node`, `pnpm`, `npx`, `cargo` and any spawned language server
/// kept running detached — still holding files, ports and console handles,
/// which made subsequent commands in the same session time out as well.
/// A no-op when the PID is unknown or already reaped.
pub fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid.filter(|p| *p != 0) else {
        return;
    };
    #[cfg(windows)]
    {
        // `taskkill` is synchronous and can take a moment; keep it off the
        // async runtime's critical path by just awaiting it inline — the
        // timeout branch is already on its way out.
        kill_pid(pid);
    }
    #[cfg(unix)]
    {
        kill_pid(pid);
    }
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        pid,
        "killed process tree"
    );
}

#[cfg(unix)]
fn configure_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setpgid(0, 0) only moves the freshly-forked child into its own
    // process group; it touches no other memory or fds.
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_cmd: &mut std::process::Command) {}

#[cfg(unix)]
fn configure_process_group_async(cmd: &mut tokio::process::Command) {
    // SAFETY: see `configure_process_group`.
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group_async(_cmd: &mut tokio::process::Command) {}

/// Configure a command so the spawned process becomes its own process-group
/// leader on Unix, allowing the whole subtree to be killed later.
pub fn prepare_tracked_command(cmd: &mut std::process::Command) {
    configure_process_group(cmd);
    hide_console(cmd);
}

/// Tokio-Command variant of [`prepare_tracked_command`].
pub fn prepare_tracked_command_async(cmd: &mut tokio::process::Command) {
    configure_process_group_async(cmd);
    hide_console_async(cmd);
}

/// Configure a std `Command` so it never allocates a visible console on Windows.
pub fn hide_console(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Configure a tokio `Command` so it never allocates a visible console on Windows.
pub fn hide_console_async(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn hide_console_sets_creation_flags() {
        let mut cmd = std::process::Command::new("cmd");
        hide_console(&mut cmd);
        // Creation flags are opaque; smoke-test only.
    }

    #[test]
    fn pid_guard_unregisters_on_drop() {
        let session = SessionId::new();
        {
            let _guard = SessionChildPidGuard::register(session, 424_242);
            let pids = session_child_pids()
                .lock()
                .unwrap()
                .get(&session)
                .cloned()
                .unwrap_or_default();
            assert!(pids.contains(&424_242));
        }
        let pids = session_child_pids().lock().unwrap().get(&session).cloned();
        assert!(pids.is_none(), "guard drop must unregister the pid");
    }
}
