use std::fs;
use std::path::Path;

use anyhow::{Context, anyhow};

use crate::cli::SignalCommand;

pub(crate) fn send_signal_from_pid_file(
    pid_path: &Path,
    signal: SignalCommand,
) -> anyhow::Result<()> {
    let raw_pid = fs::read_to_string(pid_path)
        .with_context(|| format!("failed to read pid file {}", pid_path.display()))?;
    let pid = raw_pid
        .trim()
        .parse::<i32>()
        .with_context(|| format!("invalid pid file contents in {}", pid_path.display()))?;

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
            signal_name(signal),
            pid,
            pid_path.display(),
            std::io::Error::last_os_error()
        ));
    }

    println!("signal `{}` sent to pid {} via {}", signal_name(signal), pid, pid_path.display());
    Ok(())
}

fn signal_name(signal: SignalCommand) -> &'static str {
    match signal {
        SignalCommand::Reload => "reload",
        SignalCommand::Restart => "restart",
        SignalCommand::Stop => "stop",
        SignalCommand::Quit => "quit",
    }
}
