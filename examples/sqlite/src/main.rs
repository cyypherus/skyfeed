use log::{error, info};
use regex::Regex;
use rusqlite::{params, Connection};
use skyfeed::{Config, Feed, FeedHandler, FeedResult, Post, Request, Uri};
use std::env;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

const FR_FEED: &'static str = "fr";
const MY_FEED: &'static str = "cyys-feed";

#[tokio::main]
async fn main() {
    let fr_feed_db = Connection::open("/space/feed.db").expect("Failed to open database");
    initialize_db(&fr_feed_db);
    let my_feed_db = Connection::open("/space/feed-2.db").expect("Failed to open database");
    initialize_db(&my_feed_db);

    let fr_feed_db = Arc::new(Mutex::new(fr_feed_db));
    let my_feed_db = Arc::new(Mutex::new(my_feed_db));

    let mut feed = MyFeed {
        handler: MyFeedHandler {
            fr_regex: regex::RegexBuilder::new(r"\\b(macron|le[- ]?pen|mélenchon|fillon|sarkozy|LREM|RN|gilets\s+jaunes|politique|trudeau|libéraux?|conservateurs?|bloc(?:\s+québécois)?|n(?:ouveau\s+)?parti(?:\s+démocratique)?|constitution(?:nel(?:le)?)?|scandale|gouvernement)\b|(extrême\s+(?:droite|gauche))")
                .case_insensitive(true)
                .build()
                .unwrap(),
            fr_feed_db: fr_feed_db.clone(),
            my_regex: regex::RegexBuilder::new(r"\b(trump|biden|far left|far right|republican|democrat|immigrant|woke|AI|slop|conservative|liberal|racist|homophobe|slur|xenophobe|israel|palestine|palestinian|immigration|ukraine|russia)s?\b")
                .case_insensitive(true)
                .build()
                .unwrap(),
            my_feed_db: my_feed_db.clone(),
        },
    };

    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(10));
    let cleanup_task = tokio::spawn(async move {
        loop {
            cleanup_interval.tick().await;
            cleanup_posts(&fr_feed_db).await;
            cleanup_posts(&my_feed_db).await;
        }
    });

    let publisher_did = env::var("PUBLISHER_DID").expect("PUBLISHER_DID env var not set");
    let feed_generator_hostname =
        env::var("FEED_GENERATOR_HOSTNAME").expect("FEED_GENERATOR_HOSTNAME env var not set");

    tokio::join!(
        feed.start_with_config(
            vec![FR_FEED, MY_FEED],
            Config {
                publisher_did,
                feed_generator_hostname
            },
            // Config::load_env_config(),
            ([0, 0, 0, 0], 3030)
        ),
        cleanup_task
    )
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
    fr_regex: Regex,
    fr_feed_db: Arc<Mutex<Connection>>,
    my_regex: Regex,
    my_feed_db: Arc<Mutex<Connection>>,
}

impl FeedHandler for MyFeedHandler {
    async fn insert_post(&mut self, post: Post) {
        // French feed
        let detected_language = whatlang::detect_lang(&post.text);
        let insert_sql = "INSERT OR REPLACE INTO posts (uri, text, timestamp) VALUES (?1, ?2, ?3)";

        if post.langs.iter().any(|lang| lang.contains("fr"))
            && detected_language == Some(whatlang::Lang::Fra)
            && !self.fr_regex.is_match(post.text.as_str())
            && post.labels.is_empty()
        {
            // info!("Storing french feed post {post:?}");

            self.fr_feed_db
                .lock()
                .await
                .execute(
                    insert_sql,
                    params![post.uri.0, post.text, post.timestamp.timestamp()],
                )
                .expect("Failed to insert post");
        }

        // My feed
        if post.langs.iter().any(|lang| lang.contains("en"))
            && detected_language == Some(whatlang::Lang::Eng)
            && !self.my_regex.is_match(post.text.as_str())
        {
            // info!("Storing my feed post {post:?}");

            self.my_feed_db
                .lock()
                .await
                .execute(
                    insert_sql,
                    params![post.uri.0, post.text, post.timestamp.timestamp()],
                )
                .expect("Failed to insert post");
        }
    }

    async fn delete_post(&mut self, uri: Uri) {
        let unpost_sql = "DELETE FROM posts WHERE uri = ?1";
        self.fr_feed_db
            .lock()
            .await
            .execute(unpost_sql, params![uri.0])
            .expect("Failed to delete post");
        self.my_feed_db
            .lock()
            .await
            .execute(unpost_sql, params![uri.0])
            .expect("Failed to delete post");
    }

    async fn like_post(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        let like_sql = "INSERT OR REPLACE INTO likes (post_uri, like_uri)
             SELECT ?1, ?2
             WHERE EXISTS (SELECT 1 FROM posts WHERE uri = ?1)";
        self.fr_feed_db
            .lock()
            .await
            .execute(like_sql, params![liked_post_uri.0, like_uri.0])
            .expect("Failed to like post");
        self.my_feed_db
            .lock()
            .await
            .execute(like_sql, params![liked_post_uri.0, like_uri.0])
            .expect("Failed to like post");
    }

    async fn delete_like(&mut self, like_uri: Uri) {
        let unlike_sql = "DELETE FROM likes WHERE like_uri = ?1";
        self.fr_feed_db
            .lock()
            .await
            .execute(unlike_sql, params![like_uri.0])
            .expect("Failed to delete like");
        self.my_feed_db
            .lock()
            .await
            .execute(unlike_sql, params![like_uri.0])
            .expect("Failed to delete like");
    }

    async fn serve_feed(&self, request: Request) -> FeedResult {
        info!("Serving {request:?}");

        let start_index = request
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);

        let posts_per_page = 50;
        let db = if request.feed == FR_FEED {
            self.fr_feed_db.lock().await
        } else if request.feed == MY_FEED {
            self.my_feed_db.lock().await
        } else {
            error!("Requested a nonexistent feed");
            return FeedResult {
                cursor: None,
                feed: Vec::new(),
            };
        };
        let mut stmt = db
            .prepare(
                "
                WITH ranked_posts AS (
                  SELECT
                    posts.uri,
                    posts.timestamp,
                    COUNT(likes.like_uri) AS likes
                  FROM posts
                  LEFT JOIN likes ON posts.uri = likes.post_uri
                  GROUP BY posts.uri
                  HAVING COUNT(likes.like_uri) > 0
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
                ORDER BY timestamp DESC
                LIMIT ? OFFSET ?;
                ",
            )
            .expect("Failed to prepare statement");

        let post_iter = stmt
            .query_map([posts_per_page as i64, start_index as i64], |row| {
                row.get::<_, String>(0)
            })
            .expect("Failed to query posts");

        let posts: Vec<Uri> = post_iter.filter_map(|x| x.ok()).map(Uri).collect();

        let next_cursor = if posts.len() == posts_per_page {
            Some((start_index + posts_per_page).to_string())
        } else {
            None
        };
        FeedResult {
            cursor: next_cursor,
            feed: posts,
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
