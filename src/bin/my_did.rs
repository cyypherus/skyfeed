use anyhow::Result;
use atrium_api::{
    agent::{store::MemorySessionStore, AtpAgent},
    types::string::Handle,
};
use clap::Parser;

pub const XRPC_HOST: &str = "https://bsky.social";

#[derive(Parser, Debug)]
struct Args {
    /// Your bluesky handle
    #[arg(long)]
    handle: String,

    /// An app password. https://bsky.app/settings/app-passwords
    #[arg(long)]
    app_password: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let handle = args.handle;
    let password = args.app_password;

    println!("Logging in...");

    let agent = AtpAgent::new(
        atrium_xrpc_client::reqwest::ReqwestClient::new(XRPC_HOST),
        MemorySessionStore::default(),
    );
    agent.login(handle.clone(), password).await?;

    println!("Fetching your did...");

    let publisher_did = agent
        .api
        .com
        .atproto
        .identity
        .resolve_handle(
            atrium_api::com::atproto::identity::resolve_handle::ParametersData {
                handle: Handle::new(handle.to_owned()).unwrap(),
            }
            .into(),
        )
        .await
        .unwrap()
        .did
        .clone();

    println!("Your DID is {publisher_did:?}");
    Ok(())
}
