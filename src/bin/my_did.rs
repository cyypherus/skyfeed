use anyhow::{Context, Result};
use atrium_api::{
    agent::{store::MemorySessionStore, AtpAgent},
    types::string::Handle,
};
use dotenv::dotenv;
use std::env;

pub const XRPC_HOST: &str = "https://bsky.social";

#[tokio::main]
async fn main() -> Result<()> {
    println!("Loading env...");

    dotenv().expect("Missing .env file");

    let handle = env::var("PUBLISHER_BLUESKY_HANDLE")
        .context("PUBLISHER_BLUESKY_HANDLE environment variable must be set")?;

    let password = env::var("PUBLISHER_BLUESKY_PASSWORD")
        .context("PUBLISHER_BLUESKY_PASSWORD environment variable must be set")?;

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
