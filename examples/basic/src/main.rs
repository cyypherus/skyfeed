use log::info;
use skyfeed::{Config, FeedHandler, FeedRequest, FeedResult, Post, Uri, start};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let handler = MyFeedHandler { posts: Vec::new() };
    let config = Config {
        publisher_did: "did:web:example.com".to_string(),
        feed_generator_hostname: "example.com".to_string(),
    };
    start(config, Arc::new(Mutex::new(handler)), ([0, 0, 0, 0], 3030)).await
}

#[derive(Clone)]
struct MyFeedHandler {
    posts: Vec<MyPost>,
}

#[derive(Debug, Clone)]
struct MyPost {
    post: Post,
    likes: HashSet<Uri>,
}

impl FeedHandler for MyFeedHandler {
    async fn available_feeds(&mut self) -> Vec<String> {
        vec!["Cats".to_string()]
    }

    async fn insert_post(&mut self, post: Post) {
        println!("📝 POST: {}", post.text);
        if post.text.to_lowercase().contains(" cat ") {
            const MAX_POSTS: usize = 100;

            self.posts.push(MyPost {
                post,
                likes: HashSet::new(),
            });

            if self.posts.len() > MAX_POSTS {
                self.posts.remove(0);
            }
        }
    }

    async fn delete_post(&mut self, uri: Uri) {
        println!("🗑️  DELETE POST: {}", uri.0);
        self.posts
            .retain(|post_with_likes| post_with_likes.post.uri != uri);
    }

    async fn insert_like(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        if let Some(post_with_likes) = self.posts.iter_mut().find(|p| p.post.uri == liked_post_uri)
        {
            post_with_likes.likes.insert(like_uri);
        }
    }

    async fn delete_like(&mut self, like_uri: Uri) {
        for post_with_likes in self.posts.iter_mut() {
            post_with_likes.likes.remove(&like_uri);
        }
    }

    async fn serve_feed(&self, request: FeedRequest) -> FeedResult {
        info!("Serving {request:?}");

        // Parse the cursor from the request
        let start_index = if let Some(cursor) = &request.cursor {
            cursor.parse::<usize>().unwrap_or(0)
        } else {
            0
        };

        let posts_per_page = 5;

        // Sort posts by likes
        let mut sorted_posts: Vec<_> = self.posts.iter().collect();
        sorted_posts.sort_by(|a, b| b.likes.len().cmp(&a.likes.len()));

        // Paginate posts
        let page_posts: Vec<_> = sorted_posts
            .into_iter()
            .skip(start_index)
            .take(posts_per_page)
            .cloned()
            .collect();

        // Calculate the next cursor
        let next_cursor = if start_index + posts_per_page < self.posts.len() {
            Some((start_index + posts_per_page).to_string())
        } else {
            None
        };

        FeedResult {
            cursor: next_cursor,
            feed: page_posts
                .into_iter()
                .map(|post_with_likes| post_with_likes.post.uri.clone())
                .collect(),
        }
    }
}
