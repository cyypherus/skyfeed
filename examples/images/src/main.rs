use log::info;
use chrono::{DateTime, Utc};
use skyfeed::{Embed, Feed, FeedHandler, FeedResult, Post, Request, Serializable_Post, Uri};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let mut feed = MyFeed {
        handler: MyFeedHandler {
            posts: Arc::new(Mutex::new(Vec::new())),
        },
    };
    feed.start("pics", ([0, 0, 0, 0], 3030)).await
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
    posts: Arc<Mutex<Vec<Post>>>,
}

impl FeedHandler for MyFeedHandler {

    async fn insert_post(&mut self, post: Post) {
        const MAX_POSTS: usize = 100;
        let mut posts = self.posts.lock().await;

        if let Some(Embed::Images(_)) = post.embed {
	    posts.push(post);
	}
        if posts.len() > MAX_POSTS {
            posts.remove(0);
        }
    }

    async fn delete_post(&mut self, uri: Uri) {
        self.posts
            .lock()
            .await
            .retain(|post_with_likes| post_with_likes.uri != uri);
    }

    async fn like_post(&mut self, like_uri: Uri, liked_post_uri: Uri) {}

    async fn delete_like(&mut self, like_uri: Uri) {}

    async fn serve_feed(&self, _request: Request) -> FeedResult {
	info!("Serving {_request:?}");
	
	let posts = self.posts.lock().await;
	
	FeedResult {
	    cursor: None,  // No pagination
            feed: Vec::<Uri>::new()
	}
    }

    async fn get_all_posts(&self) -> Vec<Serializable_Post> {
        let serializable_posts = self.posts
            .lock()
            .await
            .clone()
            .into_iter()
            .map(|post| Serializable_Post {
	      cid: post.cid.clone().0,
	      text: post.text.clone(),
	      timestamp: post.timestamp.timestamp_millis().to_string(),
	  })
          .collect();
        serializable_posts
    }
}
