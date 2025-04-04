use anyhow::Result;
use atrium_api::{
    agent::atp_agent::{store::MemorySessionStore, AtpAgent},
    types::string::{Handle, Nsid, RecordKey},
};
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    /// Your bluesky handle
    #[arg(long)]
    handle: String,

    /// An app password. See [app-passwords](https://bsky.app/settings/app-passwords)
    #[arg(long)]
    app_password: String,

    /// Short name of the feed. Sharing a link to a feed will use a URL like `<host>/profile/<user-did>/feed/<name!>`. This utility will unpublish the feed with the matching name.
    #[arg(long)]
    name: String,
}

pub const XRPC_HOST: &str = "https://bsky.social";

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let handle = args.handle;
    let password = args.app_password;

    let record_key = RecordKey::new(args.name.to_owned()).expect("Invalid record key name.");

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
                rkey: record_key,
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
