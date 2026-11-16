#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let runtime = rockbadger_desktop_common::start().await?;
    println!("{}", runtime.url);
    tokio::signal::ctrl_c().await?;
    Ok(())
}
