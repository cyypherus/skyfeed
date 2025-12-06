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

    // Fetch the DID document JSON
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
    for feed in describe["feeds"].as_array().expect("Unexpected object") {
        let at_uri = feed["uri"].as_str().expect("at-uri not found");
        let mut cursor: Option<String> = None;

        for page in 0..2 {
            let mut query = vec![("feed", at_uri.to_string()), ("limit", "20".to_string())];
            if let Some(ref c) = cursor {
                query.push(("cursor", c.clone()));
            }

            let skeleton_response = client
                .get(format!(
                    "{}/xrpc/app.bsky.feed.getFeedSkeleton",
                    args.local_url
                ))
                .query(&query)
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
                "Feed Skeleton Response (Page {}):\n{}",
                page + 1,
                serde_json::to_string_pretty(&skeleton).expect("Failed to pretty print skeleton JSON")
            );

            // Extract cursor for next iteration
            cursor = skeleton["cursor"]
                .as_str()
                .map(|s| s.to_string());

            if cursor.is_none() {
                break;
            }
        }
    }
}
