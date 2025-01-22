use log::info;
use skyfeed::{Feed, FeedHandler, FeedResult, Post, Request, Uri};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let mut feed = MyFeed {
        handler: MyFeedHandler {
            posts: Arc::new(Mutex::new(Vec::new())),
        },
    };
    feed.start("Cats", ([0, 0, 0, 0], 3030)).await
}

struct MyFeed {
    handler: MyFeedHandler,
}

impl Feed<MyFeedHandler> for MyFeed {
    fn handler(&mut self) -> MyFeedHandler {
        self.handler.clone()
    }
}

#[derive(Clone)]
struct MyFeedHandler {
    posts: Arc<Mutex<Vec<MyPost>>>,
}

#[derive(Debug, Clone)]
struct MyPost {
    post: Post,
    likes: HashSet<Uri>,
}

impl FeedHandler for MyFeedHandler {
    async fn insert_post(&mut self, post: Post) {
        if post.text.to_lowercase().contains(" cat ") {
            info!("Storing {post:?}");
            const MAX_POSTS: usize = 100;

            let mut posts = self.posts.lock().await;

            posts.push(MyPost {
                post,
                likes: HashSet::new(),
            });

            if posts.len() > MAX_POSTS {
                posts.remove(0);
            }
        }
    }

    async fn delete_post(&mut self, uri: Uri) {
        self.posts
            .lock()
            .await
            .retain(|post_with_likes| post_with_likes.post.uri != uri);
    }

    async fn like_post(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        if let Some(post_with_likes) = self
            .posts
            .lock()
            .await
            .iter_mut()
            .find(|p| p.post.uri == liked_post_uri)
        {
            post_with_likes.likes.insert(like_uri);
        }
    }

    async fn delete_like(&mut self, like_uri: Uri) {
        let mut posts = self.posts.lock().await;
        for post_with_likes in posts.iter_mut() {
            post_with_likes.likes.remove(&like_uri);
        }
    }

    async fn serve_feed(&self, request: Request) -> FeedResult {
        info!("Serving {request:?}");

        let posts = self.posts.lock().await;

        // Parse the cursor from the request
        let start_index = if let Some(cursor) = &request.cursor {
            cursor.parse::<usize>().unwrap_or(0)
        } else {
            0
        };

        let posts_per_page = 5;

        // Sort posts by likes
        let mut sorted_posts: Vec<_> = posts.iter().collect();
        sorted_posts.sort_by(|a, b| b.likes.len().cmp(&a.likes.len()));

        // Paginate posts
        let page_posts: Vec<_> = sorted_posts
            .into_iter()
            .skip(start_index)
            .take(posts_per_page)
            .cloned()
            .collect();

        // Calculate the next cursor
        let next_cursor = if start_index + posts_per_page < posts.len() {
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
