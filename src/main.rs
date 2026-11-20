mod audit;
mod auth;
mod config;
mod fsguard;
mod instance;
mod jarinfo;
mod logs;
mod mask;
mod mcp;
mod props;
mod rcon;
mod redact;
mod server;
mod store;
mod tls;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "abmpt", version, about = "Minecraft test server manager exposed over MCP")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the MCP service (HTTPS on ABMPT_LISTEN).
    Serve,
    /// Manage agent tokens.
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// Print the CA certificate that clients must trust.
    Ca,
}

#[derive(Subcommand)]
enum TokenCmd {
    /// Create a token for an agent; the plaintext is printed once.
    Create { name: String },
    /// Revoke an agent's token (takes effect immediately).
    Revoke { name: String },
    /// List token names.
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rmcp=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let cfg = config::Config::from_env()?;
    let paths = cfg.paths();
    match cli.cmd {
        Cmd::Serve => server::serve(cfg).await,
        Cmd::Token { cmd } => {
            paths.ensure()?;
            let store = auth::TokenStore::new(paths.tokens_file());
            match cmd {
                TokenCmd::Create { name } => {
                    let t = store.create(&name)?;
                    eprintln!("Token for '{name}' (shown only once; store it in the agent's ABMPT_TOKEN env var):");
                    println!("{t}");
                }
                TokenCmd::Revoke { name } => {
                    store.revoke(&name)?;
                    eprintln!("revoked token '{name}'");
                }
                TokenCmd::List => {
                    for t in store.list()? {
                        println!("{}\tcreated {}", t.name, t.created);
                    }
                }
            }
            Ok(())
        }
        Cmd::Ca => {
            if !cfg.tls_enabled {
                anyhow::bail!("TLS is off (ABMPT_TLS=off): there is no CA certificate to use");
            }
            paths.ensure()?;
            let f = tls::ensure(&paths.tls_dir(), &cfg.tls_sans)?;
            print!("{}", std::fs::read_to_string(f.ca_cert)?);
            Ok(())
        }
    }
}
