use std::fs;
use std::path::Path;

use anyhow::{Context, anyhow, bail};

use crate::cli::SignalCommand;
use crate::pid_file::read_pid_file_record;

pub(crate) fn send_signal_from_pid_file(
    pid_path: &Path,
    signal: SignalCommand,
) -> anyhow::Result<()> {
    let pid_record = read_pid_file_record(pid_path)?;
    let pid = pid_record.pid;

    if pid <= 0 {
        bail!("invalid pid {} in {}; expected a positive pid", pid, pid_path.display());
    }

    let expected_process_name = pid_record.process_name.unwrap_or_else(current_process_name);
    ensure_process_matches_pid_file(pid, pid_path, &expected_process_name)?;

    let signal_number = match signal {
        SignalCommand::Reload => libc::SIGHUP,
        SignalCommand::Restart => libc::SIGUSR2,
        SignalCommand::Stop => libc::SIGTERM,
        SignalCommand::Quit => libc::SIGQUIT,
    };

    let result = unsafe { libc::kill(pid, signal_number) };
    if result != 0 {
        return Err(anyhow!(
            "failed to send signal `{}` to pid {} from {}: {}",
            signal.as_str(),
            pid,
            pid_path.display(),
            std::io::Error::last_os_error()
        ));
    }

    println!("signal `{}` sent to pid {} via {}", signal.as_str(), pid, pid_path.display());
    Ok(())
}

fn ensure_process_matches_pid_file(
    pid: i32,
    pid_path: &Path,
    expected_process_name: &str,
) -> anyhow::Result<()> {
    let alive = unsafe { libc::kill(pid, 0) };
    if alive != 0 {
        return Err(anyhow!(
            "failed to verify pid {} from {} before sending signal: {}",
            pid,
            pid_path.display(),
            std::io::Error::last_os_error()
        ));
    }

    let actual_process_name = fs::read_to_string(format!("/proc/{pid}/comm"))
        .with_context(|| format!("failed to read process identity for pid {pid}"))?;
    let actual_process_name = actual_process_name.trim();

    if actual_process_name != expected_process_name {
        bail!(
            "pid identity mismatch for {}: expected process `{}`, found `{}` for pid {}",
            pid_path.display(),
            expected_process_name,
            actual_process_name,
            pid
        );
    }

    Ok(())
}

fn current_process_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().and_then(|name| name.to_str()).map(ToOwned::to_owned))
        .unwrap_or_else(|| "rginx".to_string())
}
