use atrium_api::app::bsky::feed::describe_feed_generator::{
    FeedData, OutputData as FeedGeneratorDescription,
};
use atrium_api::app::bsky::feed::get_feed_skeleton::OutputData as FeedSkeleton;
use atrium_api::app::bsky::feed::get_feed_skeleton::Parameters as FeedSkeletonQuery;
use atrium_api::app::bsky::feed::get_feed_skeleton::ParametersData as FeedSkeletonParameters;
use atrium_api::types::Object;
use env_logger::Env;
use log::{info, warn};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use warp::Filter;

use crate::config::Config;
use crate::firehose::{FirehoseConnector, FirehoseEvent};
use crate::models::FeedRequest;
use crate::utility_models::{DidDocument, Service};
use crate::{FeedResult, Post, Uri};

/// A feed handler is responsible for
/// - Storing and managing firehose input.
/// - Serving responses to feed requests with `serve_feed`
///
/// One feed handler can implement any number of feeds. Feed IDs / names are specified by the `available_feeds` function, & are later referred to in the `FeedRequest::feed` field.
pub trait FeedHandler {
    fn available_feeds(&mut self) -> impl Future<Output = Vec<String>> + Send;
    fn insert_post(&mut self, post: Post) -> impl Future<Output = ()> + Send;
    fn delete_post(&mut self, uri: Uri) -> impl Future<Output = ()> + Send;
    fn insert_like(
        &mut self,
        like_uri: Uri,
        liked_post_uri: Uri,
    ) -> impl std::future::Future<Output = ()> + Send;
    fn delete_like(&mut self, like_uri: Uri) -> impl Future<Output = ()> + Send;
    fn serve_feed(&self, request: FeedRequest) -> impl Future<Output = FeedResult> + Send;
}

/// Starts the feed generator server & connects to the firehose.
///
/// - feed_handler: An object which handles firehose input & serve feeds. This object can implement multiple feeds.
/// - config: Configuration values, see `Config`
/// - address: The address to bind the server to
///
/// # Panics
///
/// Panics if unable to bind to the provided address.
pub async fn start(
    config: Config,
    feed_handler: impl FeedHandler + Send + 'static,
    address: impl Into<SocketAddr> + Send + 'static,
) {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let address: SocketAddr = address.into();
    let feed_handler = Arc::new(Mutex::new(feed_handler));
    let config = config;

    let did_config = config.clone();
    let did_json = warp::path(".well-known")
        .and(warp::path("did.json"))
        .and(warp::get())
        .and_then(move || did_json(did_config.clone()));

    let describe_feed_config = config.clone();
    let describe_feed_generator = warp::path("xrpc")
        .and(warp::path("app.bsky.feed.describeFeedGenerator"))
        .and(warp::get())
        .and_then({
            let feed_handler = feed_handler.clone();
            move || describe_feed_generator(describe_feed_config.clone(), feed_handler.clone())
        });

    let get_feed_skeleton = warp::path("xrpc")
        .and(warp::path("app.bsky.feed.getFeedSkeleton"))
        .and(warp::get())
        .and(warp::query::<FeedSkeletonParameters>())
        .and_then({
            let feed_handler = feed_handler.clone();
            move |query: FeedSkeletonParameters| {
                get_feed_skeleton(query.into(), feed_handler.clone())
            }
        });

    let api = did_json.or(describe_feed_generator).or(get_feed_skeleton);

    info!("Serving feed on {:?}", address);

    let routes = api.with(warp::log::custom(|info| {
        let method = info.method();
        let path = info.path();
        let status = info.status();
        let elapsed = info.elapsed().as_millis();

        if status.is_success() {
            info!(
                "Method: {}, Path: {}, Status: {}, Elapsed Time: {}ms",
                method, path, status, elapsed
            );
        } else {
            log::error!(
                "Method: {}, Path: {}, Status: {}, Elapsed Time: {}ms",
                method,
                path,
                status,
                elapsed,
            );
        }
    }));
    let feed_server = warp::serve(routes);

    let (tx, rx): (flume::Sender<FirehoseEvent>, _) = flume::unbounded();

    let feed_handler = feed_handler.clone();
    let event_handler = tokio::spawn(async move {
        let mut warning_log_counter = 0usize;
        while let Ok(event) = rx.recv_async().await {
            warning_log_counter += 1;
            let waiting_updates = rx.len();
            if waiting_updates >= 2000 && warning_log_counter >= 5 {
                warning_log_counter = 0;
                warn!(
                    "{waiting_updates} updates are awaiting processing, your feed handler implementation may not be processing updates quickly enough. This will result in continuously increasing memory usage if it continues!"
                )
            }
            let mut feed_handler = feed_handler.lock().await;
            match event {
                FirehoseEvent::Post(post) => {
                    feed_handler.insert_post(*post).await;
                }
                FirehoseEvent::DeletePost(uri) => {
                    feed_handler.delete_post(uri).await;
                }
                FirehoseEvent::Like(like_uri, post_uri) => {
                    feed_handler.insert_like(like_uri, post_uri).await;
                }
                FirehoseEvent::DeleteLike(uri) => {
                    feed_handler.delete_like(uri).await;
                }
            }
        }
    });

    let firehose_listener = tokio::spawn(async move {
        if let Err(e) = FirehoseConnector::run(tx).await {
            log::error!("Firehose error: {}", e);
        }
    });

    let _ = tokio::join!(feed_server.run(address), firehose_listener, event_handler);
}

async fn did_json(config: Config) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&DidDocument {
        context: vec!["https://www.w3.org/ns/did/v1".to_owned()],
        id: format!("did:web:{}", config.feed_generator_hostname),
        service: vec![Service {
            id: "#bsky_fg".to_owned(),
            type_: "BskyFeedGenerator".to_owned(),
            service_endpoint: format!("https://{}", config.feed_generator_hostname),
        }],
    }))
}

async fn describe_feed_generator(
    config: Config,
    feed_handler: Arc<Mutex<impl FeedHandler + Send>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&FeedGeneratorDescription {
        did: atrium_api::types::string::Did::new(format!(
            "did:web:{}",
            config.feed_generator_hostname
        ))
        .unwrap(),
        feeds: feed_handler
            .lock()
            .await
            .available_feeds()
            .await
            .iter()
            .map(|name| {
                Object::from(FeedData {
                    uri: format!(
                        "at://{}/app.bsky.feed.generator/{}",
                        config.publisher_did, name
                    ),
                })
            })
            .collect(),
        links: None,
    }))
}

async fn get_feed_skeleton(
    query: FeedSkeletonQuery,
    feed_handler: Arc<Mutex<impl FeedHandler + Send>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let skeleton = feed_handler
        .lock()
        .await
        .serve_feed(FeedRequest {
            cursor: query.cursor.clone(),
            limit: query.limit.map(|l| l.into()),
            feed: query.feed.split("/").last().unwrap_or("").to_string(),
        })
        .await;
    Ok::<warp::reply::Json, warp::Rejection>(warp::reply::json(&FeedSkeleton {
        cursor: skeleton.cursor,
        feed: skeleton
            .feed
            .into_iter()
            .map(|uri| {
                Object::from(atrium_api::app::bsky::feed::defs::SkeletonFeedPostData {
                    feed_context: None,
                    post: uri.0,
                    reason: None,
                })
            })
            .collect(),
        req_id: None,
    }))
}
