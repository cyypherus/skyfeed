use log::info;
use rusqlite::{params, Connection};
use skyfeed::{Feed, FeedHandler, FeedResult, Post, Request, Uri};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let db = Connection::open("feed.db").expect("Failed to open database");
    initialize_db(&db);

    let db = Arc::new(Mutex::new(db));

    let mut feed = MyFeed {
        handler: MyFeedHandler { db: db.clone() },
    };

    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(10));
    let cleanup_task = tokio::spawn(async move {
        loop {
            cleanup_interval.tick().await;
            cleanup_posts(&db).await;
        }
    });

    tokio::join!(feed.start("Cats", ([0, 0, 0, 0], 3030)), cleanup_task)
        .1
        .expect("Starting tasks failed");
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
        let cat_regex =
            regex::RegexBuilder::new("\\b(cats?|kitty|kitties|kittens?|feline|catsofbluesky)\\b")
                .case_insensitive(true)
                .build()
                .unwrap();

        let unwanted_regex =
            regex::RegexBuilder::new("\\b(trump|kamala|harris|biden|democrats?|democratic|republicans?|politics|dems?|aoc|GOP|vance|musk|elon|walz|fascists?|furryart|smut|furries|dnc|RFK)\\b|(right wing|left wing)")
                .case_insensitive(true)
                .build()
                .unwrap();

        if cat_regex.is_match(post.text.as_str())
            && !unwanted_regex.is_match(post.text.as_str())
            && post.labels.is_empty()
        {
            info!("Storing {post:?}");
            let db = self.db.lock().await;

            db.execute(
                "INSERT OR REPLACE INTO posts (uri, text, timestamp) VALUES (?1, ?2, ?3)",
                params![post.uri.0, post.text, post.timestamp.timestamp()],
            )
            .expect("Failed to insert post");
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
                "
                WITH ranked_posts AS (
                    SELECT
                        uri,
                        timestamp,
                        COUNT(like_uri) AS likes
                    FROM posts
                    LEFT JOIN likes ON posts.uri = likes.post_uri
                    GROUP BY posts.uri
                    HAVING COUNT(like_uri) > 0
                ),
                sorted_posts AS (
                    SELECT
                        uri,
                        timestamp,
                        likes,
                        PERCENT_RANK() OVER (ORDER BY likes DESC) AS rank
                    FROM ranked_posts
                )
                SELECT uri, likes
                FROM sorted_posts
                WHERE rank <= 0.05
                ORDER BY timestamp DESC;
             ",
            )
            .expect("Failed to prepare statement");

        let post_iter = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("Failed to query posts");

        let posts: Vec<Uri> = post_iter.map(|x| x.unwrap()).map(Uri).collect();

        let start_index = request
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let posts_per_page = 50;

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
            feed: page_posts,
        }
    }
}

async fn cleanup_posts(db: &Arc<Mutex<Connection>>) {
    const MAX_POSTS: usize = 10_000;

    let cleaned_posts = db
        .lock()
        .await
        .execute(
            &format!(
                "
                DELETE FROM posts
                WHERE uri NOT IN (
                    SELECT uri
                    FROM posts
                    ORDER BY timestamp DESC
                    LIMIT {MAX_POSTS}
                );
                "
            ),
            [],
        )
        .expect("Failed to clean up old posts");

    info!("Cleaned up {cleaned_posts} posts");
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

    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_likes_post_uri ON likes(post_uri)",
        [],
    )
    .expect("Failed to create index on likes.post_uri");
}
