#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let runtime = agentgod_desktop_common::start(agentgod_desktop_common::AppMode::Local).await?;
    println!("{}", runtime.url);
    tokio::signal::ctrl_c().await?;
    Ok(())
}
