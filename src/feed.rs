use atrium_api::app::bsky::feed::describe_feed_generator::{
    FeedData, OutputData as FeedGeneratorDescription,
};
use atrium_api::app::bsky::feed::get_feed_skeleton::OutputData as FeedSkeleton;
use atrium_api::app::bsky::feed::get_feed_skeleton::Parameters as FeedSkeletonQuery;
use atrium_api::app::bsky::feed::get_feed_skeleton::ParametersData as FeedSkeletonParameters;
use atrium_api::types::Object;
use env_logger::Env;
use log::{info, warn};
use std::fmt::Debug;
use std::net::SocketAddr;
use warp::Filter;

use crate::firehose::{FirehoseConnector, FirehoseEvent};
use crate::models::Request;
use crate::utility_models::{DidDocument, Service};
use crate::{config::Config, feed_handler::FeedHandler};

/// A `Feed` stores a `FeedHandler`, handles feed server endpoints & connects to the Firehose using the `start` methods.
pub trait Feed<Handler: FeedHandler + Clone + Send + Sync + 'static> {
    fn handler(&mut self) -> Handler;
    /// Starts the feed generator server & connects to the firehose.
    ///
    /// This method loads the config from a local .env file using `dotenv`. See `Config`
    ///
    /// - feed_names: The identifying names of your feeds. This value is used in the feed URL & when identifying which feed to *publish* or *unpublish*. This is a separate value from the display name.
    /// - address: The address to bind the server to
    ///
    /// # Panics
    ///
    /// Panics if unable to bind to the provided address.
    fn start(
        &mut self,
        feed_names: Vec<&'static str>,
        address: impl Into<SocketAddr> + Debug + Clone + Send,
    ) -> impl std::future::Future<Output = ()> + Send {
        self.start_with_config(feed_names, Config::load_env_config(), address)
    }
    /// Starts the feed generator server & connects to the firehose.
    ///
    /// - feed_names: The identifying names of your feeds. This value is used in the feed URL & when identifying which feed to *publish* or *unpublish*. This is a separate value from the display name.
    /// - config: Configuration values, see `Config`
    /// - address: The address to bind the server to
    ///
    /// # Panics
    ///
    /// Panics if unable to bind to the provided address.
    fn start_with_config(
        &mut self,
        feed_names: Vec<&'static str>,
        config: Config,
        address: impl Into<SocketAddr> + Debug + Clone + Send,
    ) -> impl std::future::Future<Output = ()> + Send {
        let handler = self.handler();
        let address = address.clone();
        let feed_names = feed_names.clone();
        async move {
            env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

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
                .and_then(move || {
                    describe_feed_generator(describe_feed_config.clone(), feed_names.clone())
                });

            let get_feed_handler = handler.clone();
            let get_feed_skeleton = warp::path("xrpc")
                .and(warp::path("app.bsky.feed.getFeedSkeleton"))
                .and(warp::get())
                .and(warp::query::<FeedSkeletonParameters>())
                .and_then(move |query: FeedSkeletonParameters| {
                    get_feed_skeleton::<Handler>(query.into(), get_feed_handler.clone())
                });

            let api = did_json.or(describe_feed_generator).or(get_feed_skeleton);

            info!("Serving feed on {}", format!("{:?}", address));

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

            let event_handler = tokio::spawn(async move {
                let mut h = handler;
                let mut warning_log_counter = 0usize;
                while let Ok(event) = rx.recv_async().await {
                    warning_log_counter += 1;
                    let waiting_updates = rx.len();
                    if waiting_updates >= 100 && warning_log_counter >= 5 {
                        warning_log_counter = 0;
                        warn!(
                            "{waiting_updates} updates are awaiting processing, your feed handler implementation may not be processing updates quickly enough. This will result in continuously increasing memory usage if it continues!"
                        )
                    }
                    match event {
                        FirehoseEvent::Post(post) => {
                            h.insert_post(post).await;
                        }
                        FirehoseEvent::DeletePost(uri) => {
                            h.delete_post(uri).await;
                        }
                        FirehoseEvent::Like(like_uri, post_uri) => {
                            h.like_post(like_uri, post_uri).await;
                        }
                        FirehoseEvent::DeleteLike(uri) => {
                            h.delete_like(uri).await;
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
    }
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
    feed_names: Vec<impl AsRef<str>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&FeedGeneratorDescription {
        did: atrium_api::types::string::Did::new(format!(
            "did:web:{}",
            config.feed_generator_hostname
        ))
        .unwrap(),
        feeds: feed_names
            .iter()
            .map(|name| {
                Object::from(FeedData {
                    uri: format!(
                        "at://{}/app.bsky.feed.generator/{}",
                        config.publisher_did,
                        name.as_ref().to_string()
                    ),
                })
            })
            .collect(),
        links: None,
    }))
}

async fn get_feed_skeleton<Handler: FeedHandler>(
    query: FeedSkeletonQuery,
    handler: Handler,
) -> Result<impl warp::Reply, warp::Rejection> {
    let skeleton = handler
        .serve_feed(Request {
            cursor: query.cursor.clone(),
            feed: query.feed.split("/").last().unwrap_or("").to_string(),
            limit: query.limit,
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
