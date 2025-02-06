use atrium_api::{
    app::bsky::{
        embed::record_with_media::MainMediaRefs,
        feed::post::{RecordEmbedRefs, RecordLabelsRefs},
    },
    types::{BlobRef, LimitedNonZeroU8, Object, TypedBlobRef, Union},
};
use chrono::{DateTime, Utc};
use log::trace;
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
    pub cid: Cid,
    pub uri: Uri,
    pub text: String,
    pub labels: Vec<Label>,
    pub langs: Vec<String>,
    pub timestamp: DateTime<Utc>,
    pub embed: Option<Embed>,
}
#[derive(Debug, Clone)]
pub struct Cid(pub String);

#[derive(Debug, Clone)]
pub enum Embed {
    Images(Vec<ImageEmbed>),
    Video(VideoEmbed),
    External(ExternalEmbed),
    Quote(QuoteEmbed),
    QuoteWithMedia(QuoteEmbed, MediaEmbed),
}

#[derive(Debug, Clone)]
pub enum MediaEmbed {
    Images(Vec<ImageEmbed>),
    Video(VideoEmbed),
    External(ExternalEmbed),
}

#[derive(Debug, Clone)]
pub struct ImageEmbed {
    pub cid: Cid,
    pub alt_text: String,
    pub mime_type: String,
}

impl ImageEmbed {
    fn from_atrium(value: Object<atrium_api::app::bsky::embed::images::ImageData>) -> Option<Self> {
        let BlobRef::Typed(TypedBlobRef::Blob(blob)) = &value.image else {
            return None;
        };
        Some(ImageEmbed {
            cid: Cid(blob.r#ref.0.to_string()),
            alt_text: value.alt.clone(),
            mime_type: blob.mime_type.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct VideoEmbed {
    pub cid: Cid,
    pub alt_text: String,
}

impl VideoEmbed {
    fn from_atrium(video: Object<atrium_api::app::bsky::embed::video::MainData>) -> Option<Self> {
        let BlobRef::Typed(TypedBlobRef::Blob(blob)) = &video.video else {
            return None;
        };
        Some(VideoEmbed {
            cid: Cid(blob.r#ref.0.to_string()),
            alt_text: video.alt.clone().unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ExternalEmbed {
    pub title: String,
    pub description: String,
    pub uri: String,
    pub thumbnail: Option<Cid>,
}

impl ExternalEmbed {
    fn from_atrium(external: Object<atrium_api::app::bsky::embed::external::MainData>) -> Self {
        ExternalEmbed {
            title: external.external.title.clone(),
            description: external.external.description.clone(),
            uri: external.external.uri.clone(),
            thumbnail: external.external.thumb.clone().and_then(|thumb| {
                let BlobRef::Typed(TypedBlobRef::Blob(blob)) = &thumb else {
                    return None;
                };
                Some(Cid(blob.r#ref.0.to_string()))
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuoteEmbed {
    pub cid: Cid,
    pub uri: String,
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
    pub(crate) fn from_atrium(value: &Union<RecordEmbedRefs>) -> Option<Self> {
        match value {
            Union::Refs(e) => match e {
                RecordEmbedRefs::AppBskyEmbedImagesMain(object) => Some(Embed::Images(
                    object
                        .images
                        .clone()
                        .into_iter()
                        .filter_map(ImageEmbed::from_atrium)
                        .collect(),
                )),
                RecordEmbedRefs::AppBskyEmbedVideoMain(video) => {
                    VideoEmbed::from_atrium(*video.clone()).map(Embed::Video)
                }
                RecordEmbedRefs::AppBskyEmbedExternalMain(external) => Some(Embed::External(
                    ExternalEmbed::from_atrium(*external.clone()),
                )),
                RecordEmbedRefs::AppBskyEmbedRecordMain(quote) => {
                    let Ok(cid) = serde_json::to_string(&quote.data.record.cid) else {
                        trace!("Cid serialization failed");
                        return None;
                    };
                    Some(Embed::Quote(QuoteEmbed {
                        cid: Cid(cid),
                        uri: quote.data.record.uri.clone(),
                    }))
                }
                RecordEmbedRefs::AppBskyEmbedRecordWithMediaMain(quote_with_media) => {
                    let Union::Refs(media) = &quote_with_media.media else {
                        return None;
                    };
                    let media = match media {
                        MainMediaRefs::AppBskyEmbedImagesMain(object) => MediaEmbed::Images(
                            object
                                .images
                                .clone()
                                .into_iter()
                                .filter_map(ImageEmbed::from_atrium)
                                .collect(),
                        ),
                        MainMediaRefs::AppBskyEmbedVideoMain(object) => {
                            MediaEmbed::Video(VideoEmbed::from_atrium(*object.clone())?)
                        }
                        MainMediaRefs::AppBskyEmbedExternalMain(object) => {
                            MediaEmbed::External(ExternalEmbed::from_atrium(*object.clone()))
                        }
                    };
                    let Ok(cid) = serde_json::to_string(&quote_with_media.record.record.cid) else {
                        trace!("Cid serialization failed");
                        return None;
                    };
                    Some(Embed::QuoteWithMedia(
                        QuoteEmbed {
                            cid: Cid(cid),
                            uri: quote_with_media.record.record.uri.clone(),
                        },
                        media,
                    ))
                }
            },
            Union::Unknown(_) => None,
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
