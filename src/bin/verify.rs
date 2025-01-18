use clap::Parser;
use reqwest::Client;
use serde_json::Value;

#[derive(Parser, Debug)]
struct Args {
    /// Local URL/Port to use for requests
    /// Ex: http://0.0.0.0:3030
    #[arg(long)]
    local_url: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let client = Client::new();

    // Fetch the DID JSON
    let did_response = client
        .get(format!("{}/.well-known/did.json", args.local_url))
        .send()
        .await
        .expect(".well-known failed");

    let did_body = did_response
        .text()
        .await
        .expect("Failed to read DID response text");
    let did: Value = serde_json::from_str(&did_body).expect("Failed to parse DID JSON");

    println!(
        "DID JSON Response:\n{}",
        serde_json::to_string_pretty(&did).expect("Failed to pretty print DID JSON")
    );

    // Fetch the Feed Generator description
    let describe_response = client
        .get(format!(
            "{}/xrpc/app.bsky.feed.describeFeedGenerator",
            args.local_url
        ))
        .send()
        .await
        .expect("Feed description failed");

    let describe_body = describe_response
        .text()
        .await
        .expect("Failed to read description response text");
    let describe: Value =
        serde_json::from_str(&describe_body).expect("Failed to parse description JSON");

    println!(
        "Describe Feed Generator Response:\n{}",
        serde_json::to_string_pretty(&describe).expect("Failed to pretty print description JSON")
    );

    // Extract at-uri and fetch the feed skeleton
    let at_uri = describe["feeds"][0]["uri"]
        .as_str()
        .expect("at-uri not found");

    let skeleton_response = client
        .get(format!(
            "{}/xrpc/app.bsky.feed.getFeedSkeleton",
            args.local_url
        ))
        .query(&[("feed", at_uri), ("limit", "20")])
        .send()
        .await
        .expect("Feed skeleton failed");

    let skeleton_body = skeleton_response
        .text()
        .await
        .expect("Failed to read skeleton response text");
    let skeleton: Value =
        serde_json::from_str(&skeleton_body).expect("Failed to parse skeleton JSON");

    println!(
        "Feed Skeleton Response:\n{}",
        serde_json::to_string_pretty(&skeleton).expect("Failed to pretty print skeleton JSON")
    );
}
