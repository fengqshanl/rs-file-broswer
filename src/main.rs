#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agentgod_files::server::run_from_env().await
}
