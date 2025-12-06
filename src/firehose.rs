use atrium_api::types::Collection;
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use atrium_api::app::bsky::feed::{self, Like};
use atrium_api::com::atproto::sync::subscribe_repos::{Commit, NSID};
use atrium_api::types::CidLink;

use crate::Cid;
use crate::models::{Did, Embed, Label, Post, Uri};
use chrono::DateTime;

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

use frames::Frame;

#[derive(Debug)]
pub enum FirehoseError {
    Frame(frames::FrameError),
    WebSocket(tokio_tungstenite::tungstenite::Error),
    CarStore(String),
}

impl std::fmt::Display for FirehoseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FirehoseError::Frame(e) => write!(f, "frame error: {}", e),
            FirehoseError::WebSocket(e) => write!(f, "websocket error: {}", e),
            FirehoseError::CarStore(msg) => write!(f, "car store error: {}", msg),
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
    pub async fn run(tx: flume::Sender<FirehoseEvent>) -> Result<(), FirehoseError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let (stream, _) = connect_async(format!("wss://bsky.network/xrpc/{NSID}"))
            .await
            .map_err(FirehoseError::WebSocket)?;
        let subscription = RepoSubscription { stream };

        let (frame_tx, frame_rx) = flume::unbounded();

        let receive_task = tokio::spawn(Self::receive_frames(subscription, frame_tx));
        let parse_task = tokio::spawn(Self::parse_frames(frame_rx, tx));

        tokio::select! {
            receive_result = receive_task => {
                receive_result.map_err(|_| FirehoseError::WebSocket(
                    tokio_tungstenite::tungstenite::Error::ConnectionClosed
                ))??;
            }
            parse_result = parse_task => {
                parse_result.map_err(|_| FirehoseError::WebSocket(
                    tokio_tungstenite::tungstenite::Error::ConnectionClosed
                ))??;
            }
        }

        Ok(())
    }

    async fn receive_frames(
        mut subscription: RepoSubscription,
        frame_tx: flume::Sender<Result<Frame, FirehoseError>>,
    ) -> Result<(), FirehoseError> {
        while let Some(message) = subscription.next().await {
            if frame_tx.send_async(message).await.is_err() {
                break;
            }
        }
        Ok(())
    }

    async fn parse_frames(
        frame_rx: flume::Receiver<Result<Frame, FirehoseError>>,
        tx: flume::Sender<FirehoseEvent>,
    ) -> Result<(), FirehoseError> {
        while let Ok(message) = frame_rx.recv_async().await {
            match message {
                Ok(Frame::Message(Some(t), message)) => {
                    if t.as_str() == "#commit" {
                        match serde_ipld_dagcbor::from_reader(std::io::Cursor::new(
                            message.body.as_slice(),
                        )) {
                            Ok(commit) => {
                                if let Err(e) = Self::handle_commit(&commit, &tx).await {
                                    log::error!("Failed to handle commit: {}", e);
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to deserialize commit: {}", e);
                            }
                        }
                    }
                }
                Ok(Frame::Message(None, _msg)) => (),
                Ok(Frame::Error(e)) => {
                    log::error!("Received error frame: {e:?}");
                    break;
                }
                Err(e) => {
                    log::error!("Error receiving frames {e}");
                    return Err(e);
                }
            }
        }
        Ok(())
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
    async fn next(&mut self) -> Option<Result<Frame, FirehoseError>> {
        match self.stream.next().await {
            Some(Ok(Message::Binary(data))) => {
                let slice: &[u8] = &data;
                Some(Frame::try_from(slice).map_err(FirehoseError::Frame))
            }
            Some(Ok(_)) | None => None,
            Some(Err(e)) => Some(Err(FirehoseError::WebSocket(e))),
        }
    }
}
