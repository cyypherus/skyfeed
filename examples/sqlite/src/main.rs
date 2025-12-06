#![allow(clippy::type_complexity)]

// use dotenv::dotenv;
use log::{error, info, trace};
use regex::Regex;
use rusqlite::{Connection, params};
use skyfeed::{Config, FeedHandler, FeedRequest, FeedResult, Post, Uri};
use std::{env, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const FR_FEED: &str = "fr";
const MY_FEED: &str = "cyys-feed";

#[tokio::main]
async fn main() {
    // dotenv().expect("No .env");
    // let db = Connection::open("feed.db").expect("Failed to open database");
    let db = Connection::open("/space/feed.db").expect("Failed to open database");
    initialize_db(&db);

    let db = Arc::new(Mutex::new(db));

    let my_feed_regex = env::var("MY_FEED_REGEX").expect("Missing feed regex");
    let fr_feed_regex = env::var("FR_FEED_REGEX").expect("Missing feed regex");
    let pending_posts = Arc::new(Mutex::new(Vec::new()));
    let pending_likes = Arc::new(Mutex::new(Vec::new()));
    let handler = MyFeedHandler {
        fr_regex: regex::RegexBuilder::new(fr_feed_regex.as_str())
            .case_insensitive(true)
            .build()
            .unwrap(),
        my_regex: regex::RegexBuilder::new(my_feed_regex.as_str())
            .build()
            .unwrap(),
        db: db.clone(),
        pending_posts: pending_posts.clone(),
        pending_likes: pending_likes.clone(),
    };

    let db_clone = db.clone();
    let posts_for_flushing = pending_posts.clone();
    let likes_for_flushing = pending_likes.clone();
    let mut flush_interval = tokio::time::interval(Duration::from_secs(10));
    let flush_task = tokio::spawn(async move {
        loop {
            flush_interval.tick().await;
            flush_posts(&db.clone(), &posts_for_flushing).await;
            flush_likes(&db.clone(), &likes_for_flushing).await;
            cleanup_posts(&db_clone, FR_FEED, 10_000).await;
            cleanup_posts(&db_clone, MY_FEED, 200_000).await;
        }
    });

    let publisher_did = env::var("PUBLISHER_DID").expect("PUBLISHER_DID env var not set");
    let feed_generator_hostname =
        env::var("FEED_GENERATOR_HOSTNAME").expect("FEED_GENERATOR_HOSTNAME env var not set");

    let config = Config {
        publisher_did,
        feed_generator_hostname,
    };

    tokio::join!(
        skyfeed::start(config, handler, ([0, 0, 0, 0], 3030)),
        flush_task
    )
    .1
    .expect("Starting tasks failed");
}

#[derive(Clone)]
struct MyFeedHandler {
    fr_regex: Regex,
    my_regex: Regex,
    db: Arc<Mutex<Connection>>,
    pending_posts: Arc<Mutex<Vec<(String, String, i64, String)>>>,
    pending_likes: Arc<Mutex<Vec<(String, String)>>>,
}

impl FeedHandler for MyFeedHandler {
    async fn available_feeds(&mut self) -> Vec<String> {
        vec![FR_FEED.to_string(), MY_FEED.to_string()]
    }

    async fn insert_post(&mut self, post: Post) {
        let detected_language = whatlang::detect_lang(&post.text);
        let timestamp = post.timestamp.timestamp();
        let feed_to_insert = if post.langs.iter().any(|lang| lang.contains("fr"))
            && detected_language == Some(whatlang::Lang::Fra)
            && !self.fr_regex.is_match(post.text.as_str())
            && post.labels.is_empty()
        {
            Some(FR_FEED)
        } else if post.langs.iter().any(|lang| lang.contains("en"))
            && detected_language == Some(whatlang::Lang::Eng)
            && !self.my_regex.is_match(post.text.as_str())
        {
            Some(MY_FEED)
        } else {
            None
        };

        if let Some(feed) = feed_to_insert {
            // trace!("Queuing {} feed post {post:?}", feed);
            let mut pending = self.pending_posts.lock().await;
            pending.push((
                post.uri.0.clone(),
                post.text.clone(),
                timestamp,
                feed.to_string(),
            ));
        }
    }

    async fn delete_post(&mut self, uri: Uri) {
        let unpost_sql = "DELETE FROM posts WHERE uri = ?1";
        self.db
            .lock()
            .await
            .execute(unpost_sql, params![uri.0])
            .expect("Failed to delete post");
    }

    async fn insert_like(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        let mut pending = self.pending_likes.lock().await;
        pending.push((liked_post_uri.0.clone(), like_uri.0.clone()));
    }

    async fn delete_like(&mut self, like_uri: Uri) {
        let unlike_sql = "DELETE FROM likes WHERE like_uri = ?1";
        self.db
            .lock()
            .await
            .execute(unlike_sql, params![like_uri.0])
            .expect("Failed to delete like");
    }

    async fn serve_feed(&self, request: FeedRequest) -> FeedResult {
        info!("Serving {request:?}");

        let (hours_back, post_offset) = request
            .cursor
            .as_deref()
            .and_then(|c| {
                let parts: Vec<&str> = c.split(':').collect();
                if parts.len() == 2 {
                    Some((
                        parts.first()?.parse::<i64>().ok()?,
                        parts.get(1)?.parse::<usize>().ok()?,
                    ))
                } else {
                    None
                }
            })
            .unwrap_or((0, 0));

        let request_limit: usize = request.limit.unwrap_or(100) as usize;
        if request.feed != FR_FEED && request.feed != MY_FEED {
            error!("Requested a nonexistent feed");
            return FeedResult {
                cursor: None,
                feed: Vec::new(),
            };
        }

        let db = self.db.lock().await;
        let mut stmt = db
            .prepare(
                "
                 WITH top_posts AS (
                   SELECT
                     posts.uri,
                     posts.timestamp,
                     ROW_NUMBER() OVER (ORDER BY COUNT(likes.like_uri) DESC, posts.timestamp DESC) as rn,
                     COUNT(likes.like_uri) AS likes
                   FROM posts
                   LEFT JOIN likes ON posts.uri = likes.post_uri
                   WHERE posts.feed = ?2 
                     AND posts.timestamp >= (strftime('%s', 'now') - ((?1 + 1) * 3600))
                     AND posts.timestamp < (strftime('%s', 'now') - (?1 * 3600))
                     AND posts.timestamp < (strftime('%s', 'now') - 300)
                   GROUP BY posts.uri
                   LIMIT 100
                 )
                 SELECT uri
                 FROM top_posts
                 WHERE rn > ?3
                 ORDER BY timestamp DESC
                 LIMIT ?4
                 ",
            )
            .expect("Failed to prepare statement");

        let post_iter = stmt
            .query_map(
                params![
                    hours_back,
                    request.feed,
                    post_offset as i64,
                    request_limit as i64
                ],
                |row| row.get::<_, String>(0),
            )
            .expect("Failed to query posts");

        let posts: Vec<Uri> = post_iter.filter_map(|x| x.ok()).map(Uri).collect();

        info!("Returned {} posts for feed {}", posts.len(), request.feed);

        let next_cursor = if posts.len() == request_limit {
            let next_offset = post_offset + posts.len();
            if next_offset < 100 {
                Some(format!("{}:{}", hours_back, next_offset))
            } else {
                Some(format!("{}:0", hours_back + 1))
            }
        } else {
            None
        };

        FeedResult {
            cursor: next_cursor,
            feed: posts,
        }
    }
}

async fn flush_posts(
    db: &Arc<Mutex<Connection>>,
    pending_posts: &Arc<Mutex<Vec<(String, String, i64, String)>>>,
) {
    let mut pending = pending_posts.lock().await;
    if pending.is_empty() {
        return;
    }

    let posts_to_insert: Vec<_> = pending.drain(..).collect();
    let count = posts_to_insert.len();
    drop(pending);

    let mut db = db.lock().await;
    let tx = db.transaction().expect("Failed to start transaction");

    {
        let mut stmt = tx
            .prepare(
                "INSERT OR REPLACE INTO posts (uri, text, timestamp, feed) VALUES (?1, ?2, ?3, ?4)",
            )
            .expect("Failed to prepare statement");

        for (uri, text, timestamp, feed) in posts_to_insert {
            stmt.execute(params![uri, text, timestamp, feed])
                .expect("Failed to insert post");
        }
    }

    tx.commit().expect("Failed to commit transaction");
    trace!("Successfully flushed {} posts", count);
}

async fn flush_likes(
    db: &Arc<Mutex<Connection>>,
    pending_likes: &Arc<Mutex<Vec<(String, String)>>>,
) {
    let mut pending = pending_likes.lock().await;
    if pending.is_empty() {
        return;
    }

    let likes_to_insert: Vec<_> = pending.drain(..).collect();
    let count = likes_to_insert.len();
    drop(pending);

    let mut db = db.lock().await;
    let tx = db.transaction().expect("Failed to start transaction");

    {
        let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO likes (post_uri, like_uri) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM posts WHERE uri = ?1)"
            ).expect("Failed to prepare statement");

        for (post_uri, like_uri) in likes_to_insert {
            stmt.execute(params![post_uri, like_uri])
                .expect("Failed to insert like");
        }
    }

    tx.commit().expect("Failed to commit transaction");
    trace!("Successfully flushed {} likes", count);
}

async fn cleanup_posts(db: &Arc<Mutex<Connection>>, feed: &str, post_limit: usize) {
    let oldest_timestamp = db
        .lock()
        .await
        .query_row(
            "SELECT MIN(timestamp) FROM posts WHERE feed = ?1",
            params![feed],
            |row| row.get::<_, Option<i64>>(0),
        )
        .expect("Failed to get oldest post timestamp");

    let oldest_date = oldest_timestamp.map(|ts| {
        chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "Invalid timestamp".to_string())
    });

    let cleaned_posts = db
        .lock()
        .await
        .execute(
            "DELETE FROM posts
            WHERE feed = ?1
              AND uri NOT IN (
                SELECT uri
                FROM posts
                WHERE feed = ?1
                ORDER BY timestamp DESC
                LIMIT ?2
            );",
            params![feed, post_limit],
        )
        .expect("Failed to clean up old posts");

    let remaining_posts = db
        .lock()
        .await
        .query_row(
            "SELECT COUNT(*) FROM posts WHERE feed = ?1",
            params![feed],
            |row| row.get::<_, usize>(0),
        )
        .expect("Failed to count remaining posts");

    info!(
        "Cleaned up {cleaned_posts} posts on {feed}. Oldest post available: {}. {remaining_posts} posts remain.",
        oldest_date.unwrap_or_else(|| "No posts".to_string())
    );
}

fn initialize_db(db: &Connection) {
    db.execute(
        "CREATE TABLE IF NOT EXISTS posts (
            uri TEXT PRIMARY KEY,
            text TEXT,
            timestamp INTEGER,
            feed TEXT
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
