use anyhow::{Context, Result, bail};
use jmap_mcp::{guard::Guard, jmap::Jmap, server::Server, tools::Tools};
use rmcp::{ServiceExt, transport::stdio};

const HELP: &str = "jmap-mcp: MCP server (stdio) for a JMAP mailbox\n\n\
Environment:\n  JMAP_URL          server base URL, e.g. https://mail.example.com\n  JMAP_TOKEN        bearer API token\n  JMAP_ALLOW_WRITE  true to allow changes (default: read-only)\n\n\
Flags:\n  --allow-write     same as JMAP_ALLOW_WRITE=true\n";

#[tokio::main]
async fn main() -> Result<()> {
    // stdout belongs to the MCP transport; logs go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut allow_write = matches!(
        std::env::var("JMAP_ALLOW_WRITE").as_deref(),
        Ok("1" | "true" | "yes" | "on")
    );
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--allow-write" => allow_write = true,
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            other => bail!("unknown argument {other}\n\n{HELP}"),
        }
    }
    let url = std::env::var("JMAP_URL").context("JMAP_URL is not set")?;
    let token = std::env::var("JMAP_TOKEN").context("JMAP_TOKEN is not set")?;

    let jmap = Jmap::connect(&url, &token).await?;
    tracing::info!(account = %jmap.account, mode = if allow_write { "read-write" } else { "read-only" }, "connected");
    let service = Server::new(Tools {
        jmap,
        guard: Guard::default(),
        allow_write,
    })
    .serve(stdio())
    .await?;
    service.waiting().await?;
    Ok(())
}
