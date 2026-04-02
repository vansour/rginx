use rginx_core::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSignal {
    Reload,
    Shutdown,
}

#[cfg(unix)]
pub async fn wait_for_signal() -> Result<RuntimeSignal> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut quit = signal(SignalKind::quit())?;
    let mut hangup = signal(SignalKind::hangup())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => Ok(RuntimeSignal::Shutdown),
        _ = terminate.recv() => Ok(RuntimeSignal::Shutdown),
        _ = quit.recv() => Ok(RuntimeSignal::Shutdown),
        _ = hangup.recv() => Ok(RuntimeSignal::Reload),
    }
}

#[cfg(not(unix))]
pub async fn wait_for_signal() -> Result<RuntimeSignal> {
    tokio::signal::ctrl_c().await?;
    Ok(RuntimeSignal::Shutdown)
}
