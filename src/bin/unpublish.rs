use anyhow::{Context, Result};
use atrium_api::{
    agent::{store::MemorySessionStore, AtpAgent},
    types::string::{Handle, Nsid},
};
use clap::Parser;
use dotenv::dotenv;
use std::env;

#[derive(Parser, Debug)]
struct Args {
    /// Short name of the feed. Sharing a link to a feed will use a URL like <host>/profile/<user-did>/feed/<name!>. This utility will unpublish the feed with the matching name.
    #[arg(long)]
    name: String,
}

pub const XRPC_HOST: &str = "https://bsky.social";

#[tokio::main]
async fn main() -> Result<()> {
    println!("Loading env...");

    dotenv().expect("Missing .env file");

    let args = Args::parse();

    let handle = env::var("PUBLISHER_BLUESKY_HANDLE")
        .context("PUBLISHER_BLUESKY_HANDLE environment variable must be set")?;

    let password = env::var("PUBLISHER_BLUESKY_PASSWORD")
        .context("PUBLISHER_BLUESKY_PASSWORD environment variable must be set")?;

    // let feed_generator_did = format!("did:web:{}", env::var("FEED_GENERATOR_HOSTNAME")?);

    println!("Logging in");

    let agent = AtpAgent::new(
        atrium_xrpc_client::reqwest::ReqwestClient::new(XRPC_HOST),
        MemorySessionStore::default(),
    );
    agent.login(handle.clone(), password).await?;

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
        .unwrap();

    agent
        .api
        .com
        .atproto
        .repo
        .delete_record(
            atrium_api::com::atproto::repo::delete_record::InputData {
                collection: Nsid::new("app.bsky.feed.generator".to_owned()).unwrap(),
                repo: atrium_api::types::string::AtIdentifier::Did(
                    publisher_did.to_owned().did.clone(),
                ),
                rkey: args.name.to_owned(),
                swap_commit: None,
                swap_record: None,
            }
            .into(),
        )
        .await
        .expect("Failed to unpublish feed");

    println!("Successfully unpublished");
    Ok(())
}
