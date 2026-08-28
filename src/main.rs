use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    file_converter_gateway::run().await
}
