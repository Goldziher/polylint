//! Registry of live subprocess process groups, so an interrupt can kill them.
//!
//! Tools run in their own process group (see [`crate::tool_run`]), which a
//! terminal's Ctrl-C no longer reaches; without this the interrupt handler would
//! exit while a tool kept running detached over scratch directories it just
//! deleted.

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard};

static LIVE: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

fn live() -> MutexGuard<'static, BTreeSet<u32>> {
    LIVE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Record the process group led by `pid` as running.
pub(crate) fn register(pid: u32) {
    live().insert(pid);
}

/// Forget `pid`; call once its owner has waited for it.
pub(crate) fn unregister(pid: u32) {
    live().remove(&pid);
}

/// Kill every registered process group. Best effort, for the interrupt handler.
pub fn kill_all() {
    let taken = std::mem::take(&mut *live());
    for pid in taken {
        kill_group(pid);
    }
}

/// Kill the process group led by `pid` and the process itself.
pub(crate) fn kill_group(pid: u32) {
    #[cfg(unix)]
    if let Ok(pgid) = i32::try_from(pid) {
        // SAFETY: `kill` takes no memory operands. Tools lead their own process
        // group (`process_group(0)` at spawn), so `-pgid` signals only their tree.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
            libc::kill(pgid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn killing_a_registered_group_ends_the_child_and_unregister_forgets_it() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .expect("spawn sleep");
        register(child.id());
        assert!(live().contains(&child.id()));
        kill_group(child.id());
        let status = child.wait().expect("wait");
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        unregister(child.id());
        assert!(!live().contains(&child.id()));
    }
}
