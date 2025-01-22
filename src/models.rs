use atrium_api::types::LimitedNonZeroU8;
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct Request {
    pub cursor: Option<String>,
    pub feed: String,
    pub limit: Option<LimitedNonZeroU8<100>>,
}

#[derive(Serialize)]
pub(crate) struct Did {
    #[serde(rename = "@context")]
    pub(crate) context: Vec<String>,
    pub(crate) id: String,
    pub(crate) service: Vec<Service>,
}

#[derive(Serialize)]
pub struct Service {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) type_: String,
    #[serde(rename = "serviceEndpoint")]
    pub(crate) service_endpoint: String,
}

#[derive(Debug, Clone)]
pub struct Post {
    pub author_did: String,
    pub cid: String,
    pub uri: Uri,
    pub text: String,
    pub labels: Vec<Label>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    Hide,
    Warn,
    NoUnauthenticated,
    Porn,
    Sexual,
    GraphicMedia,
    Nudity,
    Other(String),
}

impl From<String> for Label {
    fn from(value: String) -> Self {
        match value.as_str() {
            "!hide" => Label::Hide,
            "!warn" => Label::Warn,
            "!no-unauthenticated" => Label::NoUnauthenticated,
            "porn" => Label::Porn,
            "sexual" => Label::Sexual,
            "graphic-media" => Label::GraphicMedia,
            "nudity" => Label::Nudity,
            other => Label::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Uri(pub String);

#[derive(Debug, Clone)]
pub struct FeedResult {
    pub cursor: Option<String>,
    pub feed: Vec<Uri>,
}
