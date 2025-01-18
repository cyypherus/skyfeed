use atrium_api::app::bsky::feed::describe_feed_generator::{
    FeedData, OutputData as FeedGeneratorDescription,
};
use atrium_api::app::bsky::feed::get_feed_skeleton::OutputData as FeedSkeleton;
use atrium_api::app::bsky::feed::get_feed_skeleton::Parameters as FeedSkeletonQuery;
use atrium_api::app::bsky::feed::get_feed_skeleton::ParametersData as FeedSkeletonParameters;
use atrium_api::record::KnownRecord;
use atrium_api::types::Object;
use env_logger::Env;
use jetstream_oxide::{
    events::{
        commit::{CommitData, CommitEvent, CommitInfo, CommitType},
        JetstreamEvent::Commit,
    },
    DefaultJetstreamEndpoints, JetstreamCompression, JetstreamConfig, JetstreamConnector,
};
use log::info;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use warp::Filter;

use crate::models::{Did, Post, Request, Service, Uri};
use crate::{config::Config, feed_handler::FeedHandler};

/// A `Feed` stores a `FeedHandler`, handles feed server endpoints & connects to the Firehose using the `start` methods.
pub trait Feed<Handler: FeedHandler + std::marker::Sync + std::marker::Send + 'static> {
    fn handler(&mut self) -> Arc<Mutex<Handler>>;
    /// Starts the feed generator server & connects to the firehose.
    ///
    /// This method loads the config from a local .env file using `dotenv`. See `Config`
    ///
    /// - name: The identifying name of your feed. This value is used in the feed URL & when identifying which feed to *unpublish*. This is a separate value from the display name.
    /// - address: The address to bind the server to
    ///
    /// # Panics
    ///
    /// Panics if unable to bind to the provided address.
    fn start(
        &mut self,
        name: impl AsRef<str>,
        address: impl Into<SocketAddr> + Debug + Clone + Send,
    ) -> impl std::future::Future<Output = ()> + Send {
        self.start_with_config(name, Config::load_env_config(), address)
    }
    /// Starts the feed generator server & connects to the firehose.
    ///
    /// - name: The identifying name of your feed. This value is used in the feed URL & when identifying which feed to *unpublish*. This is a separate value from the display name.
    /// - config: Configuration values, see `Config`
    /// - address: The address to bind the server to
    ///
    /// # Panics
    ///
    /// Panics if unable to bind to the provided address.
    fn start_with_config(
        &mut self,
        name: impl AsRef<str>,
        config: Config,
        address: impl Into<SocketAddr> + Debug + Clone + Send,
    ) -> impl std::future::Future<Output = ()> + Send {
        let handler = self.handler();
        let address = address.clone();
        let feed_name = name.as_ref().to_string();
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
                    describe_feed_generator(describe_feed_config.clone(), feed_name.clone())
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
            let firehose_listener = tokio::spawn(async move {
                let jetstream = JetstreamConnector::new(JetstreamConfig {
                    endpoint: DefaultJetstreamEndpoints::USEastOne.into(),
                    compression: JetstreamCompression::Zstd,
                    ..Default::default()
                })
                .unwrap();
                let (receiver, _) = jetstream.connect().await.unwrap();
                while let Ok(event) = receiver.recv_async().await {
                    if let Commit(commit) = event {
                        #[allow(clippy::collapsible_match)]
                        match commit {
                            CommitEvent::Create {
                                info,
                                commit:
                                    CommitData {
                                        info:
                                            CommitInfo {
                                                operation: CommitType::Create,
                                                collection,
                                                rkey,
                                                ..
                                            },
                                        cid,
                                        record: KnownRecord::AppBskyFeedPost(record),
                                    },
                            } => {
                                #[allow(clippy::to_string_in_format_args)]
                                let uri = format!(
                                    "at://{}/{}/{}",
                                    info.did.to_string(),
                                    collection.to_string(),
                                    rkey
                                );
                                handler
                                    .lock()
                                    .await
                                    .insert_post(Post {
                                        author_did: info.did.to_string(),
                                        cid: serde_json::to_string(&cid).unwrap(),
                                        uri: Uri(uri),
                                        text: record.text.clone(),
                                        timestamp: record.created_at.as_str().to_string(),
                                    })
                                    .await;
                            }
                            CommitEvent::Create {
                                info,
                                commit:
                                    CommitData {
                                        info:
                                            CommitInfo {
                                                operation: CommitType::Create,
                                                collection,
                                                rkey,
                                                ..
                                            },
                                        record: KnownRecord::AppBskyFeedLike(record),
                                        ..
                                    },
                            } => {
                                #[allow(clippy::to_string_in_format_args)]
                                let uri = format!(
                                    "at://{}/{}/{}",
                                    info.did.to_string(),
                                    collection.to_string(),
                                    rkey
                                );
                                handler
                                    .lock()
                                    .await
                                    .like_post(Uri(uri), Uri(record.subject.uri.clone()))
                                    .await;
                            }
                            CommitEvent::Delete {
                                info,
                                commit:
                                    CommitInfo {
                                        rkey, collection, ..
                                    },
                            } => {
                                #[allow(clippy::to_string_in_format_args)]
                                let uri = format!(
                                    "at://{}/{}/{}",
                                    info.did.to_string(),
                                    collection.to_string(),
                                    rkey
                                );
                                if collection.to_string() == "app.bsky.feed.post" {
                                    handler.lock().await.delete_post(Uri(uri)).await;
                                } else if collection.to_string() == "app.bsky.feed.like" {
                                    handler.lock().await.delete_like(Uri(uri)).await;
                                }
                            }
                            _ => (),
                        }
                    }
                }
            });

            tokio::join!(feed_server.run(address), firehose_listener)
                .1
                .expect("Couldn't await tasks");
        }
    }
}

async fn did_json(config: Config) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&Did {
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
    feed_name: String,
) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&FeedGeneratorDescription {
        did: atrium_api::types::string::Did::new(format!(
            "did:web:{}",
            config.feed_generator_hostname
        ))
        .unwrap(),
        feeds: vec![Object::from(FeedData {
            uri: format!(
                "at://{}/app.bsky.feed.generator/{}",
                config.publisher_did, feed_name
            ),
        })],
        links: None,
    }))
}

async fn get_feed_skeleton<Handler: FeedHandler>(
    query: FeedSkeletonQuery,
    handler: Arc<Mutex<Handler>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let skeleton = handler
        .lock()
        .await
        .serve_feed(Request {
            cursor: query.cursor.clone(),
            feed: query.feed.clone(),
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
    }))
}
