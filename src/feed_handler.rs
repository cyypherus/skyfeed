use crate::models::{FeedResult, Post, Request, Uri};

pub trait FeedHandler {
    fn insert_post(&mut self, post: Post) -> impl std::future::Future<Output = ()> + Send;
    fn delete_post(&mut self, uri: Uri) -> impl std::future::Future<Output = ()> + Send;
    fn like_post(
        &mut self,
        like_uri: Uri,
        liked_post_uri: Uri,
    ) -> impl std::future::Future<Output = ()> + Send;
    fn delete_like(&mut self, like_uri: Uri) -> impl std::future::Future<Output = ()> + Send;
    fn serve_feed(&self, request: Request) -> impl std::future::Future<Output = FeedResult> + Send;
}
