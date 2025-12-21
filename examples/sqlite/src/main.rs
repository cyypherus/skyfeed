// use dotenv::dotenv;
use chrono_tz::America::Denver;
use log::{error, info, trace};
use rayon::prelude::*;
use regex::Regex;
use rusqlite::{Connection, params};
use skyfeed::{Config, FeedHandler, FeedRequest, FeedResult, Post, Uri};
use std::{env, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const FR_FEED: &str = "fr";
const MY_FEED: &str = "cyys-feed";

#[derive(Clone)]
struct PendingPost {
    uri: String,
    text: String,
    timestamp: i64,
    langs: Vec<String>,
    labels: Vec<String>,
}

#[derive(Clone)]
struct PendingLike {
    post_uri: String,
    like_uri: String,
}

#[tokio::main]
async fn main() {
    // dotenv().expect("No .env");
    // let db = Connection::open("feed.db").expect("Failed to open database");
    let db = Connection::open("/space/feed.db").expect("Failed to open database");
    initialize_db(&db);

    let my_feed_regex = env::var("MY_FEED_REGEX").expect("Missing feed regex");
    let fr_feed_regex = env::var("FR_FEED_REGEX").expect("Missing feed regex");
    let handler = Arc::new(Mutex::new(MyFeedHandler {
        fr_regex: regex::RegexBuilder::new(fr_feed_regex.as_str())
            .case_insensitive(true)
            .build()
            .unwrap(),
        my_regex: regex::RegexBuilder::new(my_feed_regex.as_str())
            .build()
            .unwrap(),
        db: Arc::new(Mutex::new(db)),
        pending_posts: Vec::new(),
        pending_likes: Vec::new(),
    }));

    let handler_flush = handler.clone();
    let mut flush_interval = tokio::time::interval(Duration::from_secs(10));
    let flush_task = tokio::spawn(async move {
        loop {
            flush_interval.tick().await;
            let mut handler = handler_flush.lock().await;
            handler.flush_posts().await;
            handler.flush_likes().await;
        }
    });

    let handler_cleanup = handler.clone();
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(30));
    let cleanup_task = tokio::spawn(async move {
        loop {
            cleanup_interval.tick().await;
            let mut handler = handler_cleanup.lock().await;
            handler.cleanup_posts(FR_FEED, 5_000).await;
            handler.cleanup_posts(MY_FEED, 60_000).await;
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
        skyfeed::start(config, 5_000, handler, ([0, 0, 0, 0], 3030)),
        flush_task,
        cleanup_task,
    )
    .1
    .expect("Starting tasks failed");
}

struct MyFeedHandler {
    fr_regex: Regex,
    my_regex: Regex,
    db: Arc<Mutex<Connection>>,
    pending_posts: Vec<PendingPost>,
    pending_likes: Vec<PendingLike>,
}

impl MyFeedHandler {
    async fn flush_posts(&mut self) {
        if self.pending_posts.is_empty() {
            return;
        }

        let posts_to_filter: Vec<_> = self.pending_posts.drain(..).collect();
        let total = posts_to_filter.len();

        let fr_regex = &self.fr_regex;
        let my_regex = &self.my_regex;

        // Parallelize filtering with regex, it can be a bottleneck
        let filtered: Vec<(String, String, i64, String)> = posts_to_filter
            .into_par_iter()
            .filter_map(|post| {
                let detected_language = whatlang::detect_lang(&post.text);

                if post.langs.iter().any(|lang| lang.contains("fr"))
                    && detected_language == Some(whatlang::Lang::Fra)
                    && !fr_regex.is_match(&post.text)
                    && post.labels.is_empty()
                {
                    Some((post.uri, post.text, post.timestamp, FR_FEED.to_string()))
                } else if post.langs.iter().any(|lang| lang.contains("en"))
                    && detected_language == Some(whatlang::Lang::Eng)
                    && !my_regex.is_match(&post.text)
                {
                    Some((post.uri, post.text, post.timestamp, MY_FEED.to_string()))
                } else {
                    None
                }
            })
            .collect();

        let count = filtered.len();
        let mut db = self.db.lock().await;
        let tx = db.transaction().expect("Failed to start transaction");

        {
            let mut stmt = tx
                  .prepare(
                      "INSERT OR REPLACE INTO posts (uri, text, timestamp, feed) VALUES (?1, ?2, ?3, ?4)",
                  )
                  .expect("Failed to prepare statement");

            for (uri, text, timestamp, feed) in filtered {
                stmt.execute(params![uri, text, timestamp, feed])
                    .expect("Failed to insert post");
            }
        }

        tx.commit().expect("Failed to commit transaction");
        trace!("Filtered {} posts, inserted {} to db", total, count);
    }

    async fn flush_likes(&mut self) {
        if self.pending_likes.is_empty() {
            return;
        }

        let likes_to_insert: Vec<_> = self.pending_likes.drain(..).collect();
        let count = likes_to_insert.len();

        let mut db = self.db.lock().await;
        let tx = db.transaction().expect("Failed to start transaction");

        {
            let mut stmt = tx.prepare(
                     "INSERT OR REPLACE INTO likes (post_uri, like_uri) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM posts WHERE uri = ?1)"
                 ).expect("Failed to prepare statement");

            for like in likes_to_insert {
                stmt.execute(params![like.post_uri, like.like_uri])
                    .expect("Failed to insert like");
            }
        }

        tx.commit().expect("Failed to commit transaction");
        trace!("Successfully flushed {} likes", count);
    }

    async fn cleanup_posts(&mut self, feed: &str, post_limit: usize) {
        let db = self.db.lock().await;
        let oldest_timestamp = db
            .query_row(
                "SELECT MIN(timestamp) FROM posts WHERE feed = ?1",
                params![feed],
                |row| row.get::<_, Option<i64>>(0),
            )
            .expect("Failed to get oldest post timestamp");

        let oldest_date = oldest_timestamp.and_then(|ts| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0).map(|dt| {
                dt.with_timezone(&Denver)
                    .format("%b %d, %Y %l:%M %p")
                    .to_string()
            })
        });

        // Gate posts between 0.5-1 hours old: only keep the top 100 by likes in that range.
        // This ensures older posts have proven engagement before being retained.
        let now = chrono::Utc::now().timestamp();
        let half_hour_ago = now - 1800;
        let one_hour_ago = now - 3600;

        let engagement_gated = db
            .execute(
                "DELETE FROM posts
                  WHERE feed = ?1
                    AND timestamp >= ?3
                    AND timestamp < ?2
                    AND uri NOT IN (
                      SELECT posts.uri
                      FROM posts
                      LEFT JOIN likes ON posts.uri = likes.post_uri
                      WHERE posts.feed = ?1
                        AND posts.timestamp >= ?3
                        AND posts.timestamp < ?2
                      GROUP BY posts.uri
                      ORDER BY COUNT(likes.like_uri) DESC
                      LIMIT 100
                    );",
                params![feed, half_hour_ago, one_hour_ago],
            )
            .expect("Failed to apply engagement gate");

        let cleaned_posts = db
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

        // Delete all posts older than 2 days (2 * 24 * 60 * 60 = 172800 seconds)
        let two_days_ago = now - 172800;
        let old_posts = db
            .execute(
                "DELETE FROM posts WHERE feed = ?1 AND timestamp < ?2",
                params![feed, two_days_ago],
            )
            .expect("Failed to delete old posts");

        let remaining_posts = db
            .query_row(
                "SELECT COUNT(*) FROM posts WHERE feed = ?1",
                params![feed],
                |row| row.get::<_, usize>(0),
            )
            .expect("Failed to count remaining posts");

        info!(
            "Cleaned up {cleaned_posts} posts on {feed} (engagement gated: {engagement_gated}, old posts: {old_posts}). Oldest post available: {}. {remaining_posts} posts remain.",
            oldest_date.unwrap_or_else(|| "No posts".to_string())
        );
    }
}

impl FeedHandler for MyFeedHandler {
    async fn available_feeds(&mut self) -> Vec<String> {
        vec![FR_FEED.to_string(), MY_FEED.to_string()]
    }

    async fn insert_post(&mut self, post: Post) {
        self.pending_posts.push(PendingPost {
            uri: post.uri.0.clone(),
            text: post.text.clone(),
            timestamp: post.timestamp.timestamp(),
            langs: post.langs.clone(),
            labels: post.labels.iter().map(|l| format!("{:?}", l)).collect(),
        });
    }

    async fn delete_post(&mut self, uri: Uri) {
        let db = Arc::clone(&self.db);
        tokio::spawn(async move {
            let unpost_sql = "DELETE FROM posts WHERE uri = ?1";
            db.lock()
                .await
                .execute(unpost_sql, params![uri.0])
                .expect("Failed to delete post");
        });
    }

    async fn insert_like(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        self.pending_likes.push(PendingLike {
            post_uri: liked_post_uri.0.clone(),
            like_uri: like_uri.0.clone(),
        });
    }

    async fn delete_like(&mut self, like_uri: Uri) {
        let db = Arc::clone(&self.db);
        tokio::spawn(async move {
            let unlike_sql = "DELETE FROM likes WHERE like_uri = ?1";
            db.lock()
                .await
                .execute(unlike_sql, params![like_uri.0])
                .expect("Failed to delete like");
        });
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
        // Paginates through top 100 posts every hour
        // Posts that are <5 minutes old are excluded to give time for moderation
        // Cursor is in the format "hour:offset" where hour is the number of hours back we will query, and offset is the number of posts from that hour that this client has already seen
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

        let next_cursor = if !posts.is_empty() {
            if posts.len() == request_limit {
                let next_offset = post_offset + posts.len();
                if next_offset < 100 {
                    Some(format!("{}:{}", hours_back, next_offset))
                } else {
                    Some(format!("{}:0", hours_back + 1))
                }
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
