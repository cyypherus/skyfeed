use atrium_api::com::atproto::repo::put_record::InputData;
use atrium_api::{
    agent::{store::MemorySessionStore, AtpAgent},
    app::bsky::feed::generator::RecordData,
    types::{
        string::{Datetime, Did, Handle, Nsid},
        TryIntoUnknown,
    },
};
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    /// Short name of the feed.
    #[arg(long)]
    name: String,

    /// Name that will be displayed in Bluesky interface
    #[arg(long)]
    display_name: String,

    /// Description that will be displayed in Bluesky interface
    #[arg(long)]
    description: String,

    /// Filename of the avatar that will be displayed
    #[arg(long)]
    avatar_filename: Option<String>,

    /// Your bluesky handle
    #[arg(long)]
    handle: String,

    /// An app password. See [app-passwords](https://bsky.app/settings/app-passwords)
    #[arg(long)]
    app_password: String,

    /// Your feed's hostname. The public-facing domain name where the service is accessible
    /// For example, the `my.domain.name` in `https://my.domain.name`
    #[arg(long)]
    hostname: String,
}

pub const XRPC_HOST: &str = "https://bsky.social";

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let handle = args.handle;
    let password = args.app_password;
    let feed_host_name = args.hostname;

    println!("Logging in...");

    let agent = AtpAgent::new(
        atrium_xrpc_client::reqwest::ReqwestClient::new(XRPC_HOST),
        MemorySessionStore::default(),
    );
    agent
        .login(handle.clone(), password)
        .await
        .expect("Login failed");

    println!("Fetching your DID...");

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

    let mut avatar = None;
    if let Some(path) = args.avatar_filename {
        println!("Uploading avatar image...");
        let bytes = std::fs::read(path).expect("Couldn't read specified avatar file");
        avatar = Some(
            agent
                .api
                .com
                .atproto
                .repo
                .upload_blob(bytes)
                .await
                .expect("Avatar upload failed"),
        );
        println!("Uploaded avatar");
    }

    println!("Publishing feed...");

    agent
        .api
        .com
        .atproto
        .repo
        .put_record(
            InputData {
                collection: Nsid::new("app.bsky.feed.generator".to_owned()).unwrap(),
                record: RecordData {
                    accepts_interactions: None,
                    #[allow(unreachable_code)]
                    avatar: avatar.map(|a| a.blob.clone()),
                    content_mode: None,
                    created_at: Datetime::now(),
                    description: Some(args.description.to_owned()),
                    description_facets: None,
                    did: Did::new(format!("did:web:{}", feed_host_name)).unwrap(),
                    display_name: args.display_name.to_owned(),
                    labels: None,
                }
                .try_into_unknown()
                .unwrap(),
                repo: atrium_api::types::string::AtIdentifier::Did(
                    publisher_did.to_owned().did.clone(),
                ),
                rkey: args.name.to_owned(),
                swap_commit: None,
                swap_record: None,
                validate: None,
            }
            .into(),
        )
        .await
        .expect("Publishing failed");

    println!("Successfully published");
}
