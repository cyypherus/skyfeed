use atrium_api::{
    app::bsky::feed::post::{RecordEmbedRefs, RecordLabelsRefs},
    types::{LimitedNonZeroU8, Union},
};
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
    pub embed: Option<Embed>,
}

#[derive(Debug, Clone)]
pub enum Embed {
    Images(Vec<ImageEmbed>),
    Video,
    External,
    Record,
    RecordWithMedia,
}

#[derive(Debug, Clone)]
pub struct ImageEmbed {
    pub alt_text: String,
    pub mime_type: String,
}

impl Label {
    pub(crate) fn from_atrium(value: &Union<RecordLabelsRefs>) -> Option<Vec<Label>> {
        match value {
            Union::Refs(refs) => match refs {
                RecordLabelsRefs::ComAtprotoLabelDefsSelfLabels(object) => Some(
                    object
                        .values
                        .clone()
                        .into_iter()
                        .map(|label| Label::from(label.val.clone()))
                        .collect::<Vec<Label>>(),
                ),
            },
            Union::Unknown(_) => None,
        }
    }
}

impl Embed {
    pub(crate) fn from_atrium(
        value: &atrium_api::types::Union<atrium_api::app::bsky::feed::post::RecordEmbedRefs>,
    ) -> Option<Self> {
        match value {
            Union::Refs(e) => match e {
                RecordEmbedRefs::AppBskyEmbedImagesMain(object) => Some(Embed::Images(
                    object
                        .images
                        .iter()
                        .map(|i| ImageEmbed {
                            alt_text: i.alt.clone(),
                            mime_type: {
                                match &i.data.image {
                                    atrium_api::types::BlobRef::Typed(typed_blob_ref) => {
                                        match typed_blob_ref {
                                            atrium_api::types::TypedBlobRef::Blob(b) => {
                                                b.mime_type.clone()
                                            }
                                        }
                                    }
                                    atrium_api::types::BlobRef::Untyped(un_typed_blob_ref) => {
                                        un_typed_blob_ref.mime_type.clone()
                                    }
                                }
                            },
                        })
                        .collect(),
                )),
                RecordEmbedRefs::AppBskyEmbedVideoMain(_) => Some(Embed::Video),
                RecordEmbedRefs::AppBskyEmbedExternalMain(_) => Some(Embed::External),
                RecordEmbedRefs::AppBskyEmbedRecordMain(_) => Some(Embed::Record),
                RecordEmbedRefs::AppBskyEmbedRecordWithMediaMain(_) => Some(Embed::RecordWithMedia),
            },
            atrium_api::types::Union::Unknown(_) => None,
        }
    }
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
