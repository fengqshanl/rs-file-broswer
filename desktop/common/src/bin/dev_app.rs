#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let runtime = agentgod_desktop_common::start().await?;
    println!("{}", runtime.url);
    tokio::signal::ctrl_c().await?;
    Ok(())
}
