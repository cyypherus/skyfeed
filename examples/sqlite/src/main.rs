use log::info;
use rusqlite::{params, Connection};
use skyfeed::{Feed, FeedHandler, FeedResult, Post, Request, Uri};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let db = Connection::open("feed.db").expect("Failed to open database");
    initialize_db(&db);

    let mut feed = MyFeed {
        handler: MyFeedHandler {
            db: Arc::new(Mutex::new(db)),
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
    db: Arc<Mutex<Connection>>,
}

impl FeedHandler for MyFeedHandler {
    async fn insert_post(&mut self, post: Post) {
        if post.text.to_lowercase().contains(" cat ") && post.labels.is_empty() {
            info!("Storing {post:?}");
            let db = self.db.lock().await;

            db.execute(
                "INSERT INTO posts (uri, text, timestamp) VALUES (?1, ?2, ?3)",
                params![post.uri.0, post.text, post.timestamp.timestamp()],
            )
            .expect("Failed to insert post");

            // Clean up old posts
            const MAX_POSTS: usize = 100;
            db.execute(
                &format!(
                    "DELETE FROM posts WHERE uri NOT IN (
                        SELECT uri FROM posts ORDER BY timestamp DESC LIMIT {MAX_POSTS}
                    )"
                ),
                [],
            )
            .expect("Failed to clean up old posts");
        }
    }

    async fn delete_post(&mut self, uri: Uri) {
        let db = self.db.lock().await;
        db.execute("DELETE FROM posts WHERE uri = ?1", params![uri.0])
            .expect("Failed to delete post");
    }

    async fn like_post(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO likes (post_uri, like_uri)
             SELECT ?1, ?2
             WHERE EXISTS (SELECT 1 FROM posts WHERE uri = ?1)",
            params![liked_post_uri.0, like_uri.0],
        )
        .expect("Failed to like post");
    }

    async fn delete_like(&mut self, like_uri: Uri) {
        let db = self.db.lock().await;
        db.execute("DELETE FROM likes WHERE like_uri = ?1", params![like_uri.0])
            .expect("Failed to delete like");
    }

    async fn serve_feed(&self, request: Request) -> FeedResult {
        info!("Serving {request:?}");

        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "SELECT uri, text, COUNT(like_uri) as likes \
             FROM posts \
             LEFT JOIN likes ON posts.uri = likes.post_uri \
             GROUP BY posts.uri \
             ORDER BY likes DESC",
            )
            .expect("Failed to prepare statement");

        let post_iter = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("Failed to query posts");

        let posts: Vec<(Uri, String)> = post_iter
            .map(|x| x.unwrap())
            .map(|x| (Uri(x.0), x.1))
            .collect();

        let start_index = request
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let posts_per_page = 5;

        let page_posts: Vec<_> = posts
            .iter()
            .skip(start_index)
            .take(posts_per_page)
            .cloned()
            .collect();

        let next_cursor = if start_index + posts_per_page < posts.len() {
            Some((start_index + posts_per_page).to_string())
        } else {
            None
        };

        FeedResult {
            cursor: next_cursor,
            feed: page_posts.iter().map(|(uri, _)| uri.clone()).collect(),
        }
    }
}

fn initialize_db(db: &Connection) {
    db.execute(
        "CREATE TABLE IF NOT EXISTS posts (
            uri TEXT PRIMARY KEY,
            text TEXT,
            timestamp INTEGER
        )",
        [],
    )
    .expect("Failed to create posts table");

    db.execute(
        "CREATE TABLE IF NOT EXISTS likes (
            post_uri TEXT,
            like_uri TEXT,
            PRIMARY KEY (post_uri, like_uri),
            FOREIGN KEY (post_uri) REFERENCES posts(uri) ON DELETE CASCADE
        )",
        [],
    )
    .expect("Failed to create likes table");
}
