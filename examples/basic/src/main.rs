use log::info;
use skyfeed::{Feed, FeedHandler, FeedResult, Post, Request, Uri};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let mut feed = MyFeed {
        handler: Arc::new(Mutex::new(MyFeedHandler {
            posts: HashMap::new(),
            likes: HashMap::new(),
        })),
    };
    feed.start(([0, 0, 0, 0], 3030)).await
}

struct MyFeed {
    handler: Arc<Mutex<MyFeedHandler>>,
}

impl Feed<MyFeedHandler> for MyFeed {
    fn handler(&mut self) -> Arc<Mutex<MyFeedHandler>> {
        self.handler.clone()
    }
}

struct MyFeedHandler {
    posts: HashMap<Uri, Post>,
    likes: HashMap<Uri, Uri>,
}

impl FeedHandler for MyFeedHandler {
    async fn insert_post(&mut self, post: Post) {
        // info!("Creating {post:?}");
        self.posts.insert(post.uri.clone(), post);
    }
    async fn delete_post(&mut self, uri: Uri) {
        self.posts.remove(&uri);
    }
    async fn like_post(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        self.likes.insert(like_uri, liked_post_uri);
    }
    async fn delete_like(&mut self, like_uri: Uri) {
        self.likes.remove(&like_uri);
    }
    async fn serve_feed(&self, request: Request) -> FeedResult {
        info!("Serving {request:?}");
        let mut post_likes: HashMap<&Uri, u32> = HashMap::new();

        for liked_post_uri in self.likes.values() {
            *post_likes.entry(liked_post_uri).or_insert(0) += 1;
        }

        let mut top_posts: Vec<_> = self.posts.values().collect();
        top_posts.sort_by(|a, b| {
            let likes_a = post_likes.get(&a.uri).unwrap_or(&0);
            let likes_b = post_likes.get(&b.uri).unwrap_or(&0);
            likes_b.cmp(likes_a)
        });

        let top_5_posts: Vec<_> = top_posts.into_iter().take(5).collect();

        FeedResult {
            cursor: None,
            feed: top_5_posts
                .into_iter()
                .map(|post| post.uri.clone())
                .collect(),
        }
    }
}
