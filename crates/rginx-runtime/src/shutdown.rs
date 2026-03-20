use rginx_core::Result;

pub async fn wait_for_signal() -> Result<()> {
    tokio::signal::ctrl_c().await?;
    Ok(())
}
