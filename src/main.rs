#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rockbadger::server::run_from_env().await
}
