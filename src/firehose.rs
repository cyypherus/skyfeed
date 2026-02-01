use crate::Cid;
use crate::firehose::frames::Frame;
use crate::models::{Did, Embed, Label, Post, ReplyRef, Uri};
use crate::update_counter::UpdatesCounter;
use atrium_api::app::bsky::feed::{self, Like};
use atrium_api::com::atproto::sync::subscribe_repos::{Commit, NSID};
use atrium_api::types::CidLink;
use atrium_api::types::Collection;
use chrono::DateTime;
use flume::RecvError;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

mod frames {
    use ipld_core::ipld::Ipld;
    use std::io::Cursor;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum FrameError {
        InvalidFrameType,
        DecodeError,
    }

    impl std::fmt::Display for FrameError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                FrameError::InvalidFrameType => write!(f, "invalid frame type"),
                FrameError::DecodeError => write!(f, "decode error"),
            }
        }
    }

    impl std::error::Error for FrameError {}

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum FrameHeader {
        Message(Option<String>),
        Error,
    }

    impl TryFrom<Ipld> for FrameHeader {
        type Error = FrameError;

        fn try_from(value: Ipld) -> Result<Self, <FrameHeader as TryFrom<Ipld>>::Error> {
            if let Ipld::Map(map) = value
                && let Some(Ipld::Integer(i)) = map.get("op")
            {
                match i {
                    1 => {
                        let t = if let Some(Ipld::String(s)) = map.get("t") {
                            Some(s.clone())
                        } else {
                            None
                        };
                        return Ok(FrameHeader::Message(t));
                    }
                    -1 => return Ok(FrameHeader::Error),
                    _ => {}
                }
            }
            Err(FrameError::InvalidFrameType)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Frame {
        Message(Option<String>, MessageFrame),
        Error(ErrorFrame),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct MessageFrame {
        pub body: Vec<u8>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ErrorFrame {}

    impl TryFrom<&[u8]> for Frame {
        type Error = FrameError;

        fn try_from(value: &[u8]) -> Result<Self, <Frame as TryFrom<&[u8]>>::Error> {
            let mut cursor = Cursor::new(value);
            let (left, right) = match serde_ipld_dagcbor::from_reader::<Ipld, _>(&mut cursor) {
                Err(serde_ipld_dagcbor::DecodeError::TrailingData) => {
                    value.split_at(cursor.position() as usize)
                }
                _ => {
                    return Err(FrameError::InvalidFrameType);
                }
            };
            let header = FrameHeader::try_from(
                serde_ipld_dagcbor::from_slice::<Ipld>(left)
                    .map_err(|_| FrameError::DecodeError)?,
            )?;
            if let FrameHeader::Message(t) = &header {
                Ok(Frame::Message(
                    t.clone(),
                    MessageFrame {
                        body: right.to_vec(),
                    },
                ))
            } else {
                Ok(Frame::Error(ErrorFrame {}))
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn serialized_data(s: &str) -> Vec<u8> {
            assert!(s.len().is_multiple_of(2));
            let b2u = |b: u8| match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                _ => unreachable!(),
            };
            s.as_bytes()
                .chunks(2)
                .map(|b| (b2u(b[0]) << 4) + b2u(b[1]))
                .collect()
        }

        #[test]
        fn deserialize_message_frame_header() {
            let data = serialized_data("a2626f700161746723636f6d6d6974");
            let ipld =
                serde_ipld_dagcbor::from_slice::<Ipld>(&data).expect("failed to deserialize");
            let result = FrameHeader::try_from(ipld);
            assert_eq!(
                result.expect("failed to deserialize"),
                FrameHeader::Message(Some(String::from("#commit")))
            );
        }

        #[test]
        fn deserialize_error_frame_header() {
            let data = serialized_data("a1626f7020");
            let ipld =
                serde_ipld_dagcbor::from_slice::<Ipld>(&data).expect("failed to deserialize");
            let result = FrameHeader::try_from(ipld);
            assert_eq!(result.expect("failed to deserialize"), FrameHeader::Error);
        }

        #[test]
        fn deserialize_invalid_frame_header() {
            {
                let data = serialized_data("a2626f700261746723636f6d6d6974");
                let ipld =
                    serde_ipld_dagcbor::from_slice::<Ipld>(&data).expect("failed to deserialize");
                let result = FrameHeader::try_from(ipld);
                assert_eq!(
                    result.expect_err("must be failed"),
                    FrameError::InvalidFrameType
                );
            }
            {
                let data = serialized_data("a1626f7021");
                let ipld =
                    serde_ipld_dagcbor::from_slice::<Ipld>(&data).expect("failed to deserialize");
                let result = FrameHeader::try_from(ipld);
                assert_eq!(
                    result.expect_err("must be failed"),
                    FrameError::InvalidFrameType
                );
            }
        }
    }
}

#[derive(Debug)]
pub enum FirehoseError {
    FrameError(frames::FrameError),
    ErrorFrame,
    WebSocket(tokio_tungstenite::tungstenite::Error),
    CarStore(String),
    SendError(String),
    RecvError(RecvError),
    JoinError(String),
    StreamClosed,
}

impl std::fmt::Display for FirehoseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FirehoseError::FrameError(e) => write!(f, "frame error: {}", e),
            FirehoseError::ErrorFrame => write!(f, "error frame"),
            FirehoseError::WebSocket(e) => write!(f, "websocket error: {}", e),
            FirehoseError::CarStore(msg) => write!(f, "car store error: {}", msg),
            FirehoseError::SendError(msg) => write!(f, "send error: {}", msg),
            FirehoseError::RecvError(e) => write!(f, "receive error: {}", e),
            FirehoseError::JoinError(msg) => write!(f, "join error: {}", msg),
            FirehoseError::StreamClosed => write!(f, "stream closed"),
        }
    }
}

impl std::error::Error for FirehoseError {}

pub enum FirehoseEvent {
    Post(Box<Post>),
    DeletePost(Uri),
    Like(Uri, Uri),
    DeleteLike(Uri),
}

pub struct FirehoseConnector;

impl FirehoseConnector {
    pub async fn run(
        endpoint: &str,
        tx: flume::Sender<FirehoseEvent>,
    ) -> Result<(), FirehoseError> {
        let cursor = Arc::new(AtomicI64::new(0));
        const MAX_ATTEMPTS: u32 = 10;
        const MAX_BACKOFF_SECS: u64 = 30;
        const RESET_THRESHOLD_SECS: u64 = MAX_BACKOFF_SECS * 10;

        let mut reconnect_attempts = 0u32;
        let mut last_reconnect = Instant::now();

        loop {
            let cursor_value = cursor.load(Ordering::Relaxed);
            let url = if cursor_value > 0 {
                format!("wss://{endpoint}/xrpc/{NSID}?cursor={}", cursor_value)
            } else {
                format!("wss://{endpoint}/xrpc/{NSID}")
            };

            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

            match Self::connect_and_run(&url, tx.clone(), cursor.clone()).await {
                Ok(()) => {}
                Err(e) => {
                    // If the last reconnect happened longer than 10x our max backoff, we can consider it irrelevant to our current reconnect process
                    if last_reconnect.elapsed() > Duration::from_secs(RESET_THRESHOLD_SECS) {
                        reconnect_attempts = 0;
                    }

                    reconnect_attempts += 1;
                    last_reconnect = Instant::now();
                    log::warn!("Firehose connection failed: {}, retrying...", e);

                    if reconnect_attempts >= MAX_ATTEMPTS {
                        log::error!(
                            "Max reconnection attempts ({}) reached, giving up",
                            MAX_ATTEMPTS
                        );
                        return Err(e);
                    }

                    // Exponential backoff with cap
                    let backoff_secs =
                        std::cmp::min(2_u64.pow(reconnect_attempts - 1), MAX_BACKOFF_SECS);
                    log::info!(
                        "Reconnecting to firehose (attempt {}), waiting {}s",
                        reconnect_attempts,
                        backoff_secs
                    );
                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                }
            }
        }
    }

    async fn connect_and_run(
        url: &str,
        tx: flume::Sender<FirehoseEvent>,
        cursor: Arc<AtomicI64>,
    ) -> Result<(), FirehoseError> {
        let (stream, _) = connect_async(url).await.map_err(FirehoseError::WebSocket)?;
        let subscription = RepoSubscription { stream };

        let (frame_tx, frame_rx) = flume::unbounded();

        let receive_task = tokio::spawn(Self::receive_frames(subscription, frame_tx));
        let parse_task = tokio::spawn(Self::parse_frames(frame_rx, tx, cursor));

        tokio::select! {
            receive_result = receive_task => {
                match receive_result
                    .map_err(|e| FirehoseError::JoinError(e.to_string()))?? {
                        // Infallible! Cool!
                    }
            }
            parse_result = parse_task => {
                match parse_result
                    .map_err(|e| FirehoseError::JoinError(e.to_string()))?? {
                        // Also infallible! Also cool!
                    }
            }
        }
    }

    async fn receive_frames(
        mut subscription: RepoSubscription,
        frame_tx: flume::Sender<Result<Frame, FirehoseError>>,
    ) -> Result<Infallible, FirehoseError> {
        loop {
            if let Some(frame) = subscription.next().await? {
                frame_tx
                    .send_async(Ok(frame))
                    .await
                    .map_err(|e| FirehoseError::SendError(e.to_string()))?;
            }
        }
    }

    async fn parse_frames(
        frame_rx: flume::Receiver<Result<Frame, FirehoseError>>,
        tx: flume::Sender<FirehoseEvent>,
        cursor: Arc<AtomicI64>,
    ) -> Result<Infallible, FirehoseError> {
        let counter = UpdatesCounter::new();
        loop {
            match frame_rx
                .recv_async()
                .await
                .map_err(FirehoseError::RecvError)?
            {
                Ok(Frame::Message(Some(t), message)) => {
                    if t.as_str() == "#commit" {
                        match serde_ipld_dagcbor::from_reader::<Commit, _>(std::io::Cursor::new(
                            message.body.as_slice(),
                        )) {
                            Ok(commit) => {
                                cursor.store(commit.seq, Ordering::Relaxed);
                                if let Err(e) = Self::handle_commit(&commit, &tx).await {
                                    log::error!("Failed to handle commit: {}", e);
                                }
                                counter.increment_and_maybe_log().await;
                            }
                            Err(e) => {
                                log::error!("Failed to deserialize commit: {}", e);
                            }
                        }
                    }
                }
                Ok(Frame::Message(None, _msg)) => (),
                Ok(Frame::Error(_)) => return Err(FirehoseError::ErrorFrame),
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    async fn handle_commit(
        commit: &Commit,
        tx: &flume::Sender<FirehoseEvent>,
    ) -> Result<(), FirehoseError> {
        let mut blocks = commit.blocks.as_slice();
        let (items, _) = rs_car::car_read_all(&mut blocks, true)
            .await
            .map_err(|e| FirehoseError::CarStore(e.to_string()))?;

        for op in &commit.ops {
            let mut s = op.path.split('/');
            let collection = s.next().expect("op.path is empty");
            let rkey = s.next().expect("no record key");
            let action = op.action.as_str();

            match (collection, action) {
                (feed::Post::NSID, "create") => {
                    if let Some((_, item_data)) = items.iter().find(|(cid, _)| {
                        let converted_cid = match cid.to_string().parse() {
                            Ok(parsed) => CidLink(parsed),
                            Err(_) => return false,
                        };
                        Some(converted_cid) == op.cid
                    }) {
                        match serde_ipld_dagcbor::from_reader(&mut item_data.clone().as_slice()) {
                            Ok(record) => {
                                let record: feed::post::Record = record;
                                let uri = format!(
                                    "at://{}/{}/{}",
                                    commit.repo.as_str(),
                                    collection,
                                    rkey
                                );

                                let timestamp =
                                    DateTime::parse_from_rfc3339(record.created_at.as_str())
                                        .ok()
                                        .map(|dt| dt.with_timezone(&chrono::Utc))
                                        .unwrap_or_else(chrono::Utc::now);

                                let cid_str = match serde_json::to_string(&op.cid) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        log::error!("Failed to serialize CID for {}: {}", rkey, e);
                                        continue;
                                    }
                                };
                                let post = Post {
                                    author_did: Did(commit.repo.as_str().to_string()),
                                    cid: Cid(cid_str),
                                    uri: Uri(uri),
                                    text: record.text.clone(),
                                    labels: record
                                        .labels
                                        .as_ref()
                                        .and_then(Label::from_atrium)
                                        .unwrap_or_default(),
                                    timestamp,
                                    embed: record.embed.as_ref().and_then(Embed::from_atrium),
                                    langs: record
                                        .langs
                                        .iter()
                                        .filter_map(|lang| serde_json::to_string(&lang).ok())
                                        .collect(),
                                    reply: record.reply.as_ref().map(|r| ReplyRef {
                                        parent: Uri(r.parent.uri.clone()),
                                        root: Uri(r.root.uri.clone()),
                                    }),
                                };
                                let _ = tx.send_async(FirehoseEvent::Post(Box::new(post))).await;
                            }
                            Err(_) => {
                                log::error!("Failed to deserialize post record for {}", rkey);
                            }
                        }
                    }
                }
                (feed::Post::NSID, "delete") => {
                    let uri = format!("at://{}/{}/{}", commit.repo.as_str(), collection, rkey);
                    let _ = tx.send_async(FirehoseEvent::DeletePost(Uri(uri))).await;
                }
                (Like::NSID, "create") => {
                    if let Some((_, item_data)) = items.iter().find(|(cid, _)| {
                        let converted_cid = match cid.to_string().parse() {
                            Ok(parsed) => CidLink(parsed),
                            Err(_) => return false,
                        };
                        Some(converted_cid) == op.cid
                    }) {
                        match serde_ipld_dagcbor::from_reader(&mut item_data.clone().as_slice()) {
                            Ok(record) => {
                                let record: feed::like::Record = record;
                                let uri = format!(
                                    "at://{}/{}/{}",
                                    commit.repo.as_str(),
                                    collection,
                                    rkey
                                );
                                let _ = tx
                                    .send_async(FirehoseEvent::Like(
                                        Uri(uri),
                                        Uri(record.subject.uri.clone()),
                                    ))
                                    .await;
                            }
                            Err(_) => {
                                log::error!("Failed to deserialize like record for {}", rkey);
                            }
                        }
                    }
                }
                (Like::NSID, "delete") => {
                    let uri = format!("at://{}/{}/{}", commit.repo.as_str(), collection, rkey);
                    let _ = tx.send_async(FirehoseEvent::DeleteLike(Uri(uri))).await;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

struct RepoSubscription {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl RepoSubscription {
    async fn next(&mut self) -> Result<Option<Frame>, FirehoseError> {
        match self.stream.next().await {
            Some(Ok(Message::Binary(data))) => {
                let slice: &[u8] = &data;
                Ok(Some(
                    Frame::try_from(slice).map_err(FirehoseError::FrameError)?,
                ))
            }
            Some(Ok(_)) => Ok(None),
            None => Err(FirehoseError::StreamClosed),
            Some(Err(e)) => Err(FirehoseError::WebSocket(e)),
        }
    }
}
