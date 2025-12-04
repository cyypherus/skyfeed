use log::{error, info, trace};
use regex::Regex;
use rusqlite::{Connection, params};
use skyfeed::{Config, Feed, FeedHandler, FeedResult, Post, Request, Uri};
use std::{env, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const FR_FEED: &str = "fr";
const MY_FEED: &str = "cyys-feed";

#[tokio::main]
async fn main() {
    // let db = Connection::open("feed.db").expect("Failed to open database");
    let db = Connection::open("/space/feed.db").expect("Failed to open database");
    initialize_db(&db);

    let db = Arc::new(Mutex::new(db));

    let pending_posts = Arc::new(Mutex::new(Vec::new()));
    let pending_likes = Arc::new(Mutex::new(Vec::new()));
    let mut feed = MyFeed {
        handler: MyFeedHandler {
            fr_regex: regex::RegexBuilder::new(r"\\b(macron|le[- ]?pen|mélenchon|fillon|sarkozy|LREM|RN|gilets\s+jaunes|politique|trudeau|libéraux?|conservateurs?|bloc(?:\s+québécois)?|n(?:ouveau\s+)?parti(?:\s+démocratique)?|constitution(?:nel(?:le)?)?|scandale|gouvernement)\b|(extrême\s+(?:droite|gauche))")
                .case_insensitive(true)
                .build()
                .unwrap(),
            my_regex: regex::RegexBuilder::new(r"\b(?i:trump|biden|far[ ]?(left|right)|(left|right)[ ]?wing|republican|(un)?democrat(ic)?|immigration|immigrant|woke|AI|slop|conservative|liberal|racis(t|m)|homophob(e|ic|ia)|xenophob(e|ic|ia)|transphob(e|ic|ia)|slur|israel[i]?|palestin(e|ian)|ukrain(e|ian)|russia(n)?|tech bro|kamala|harris|politic(ian|al)|communis(m|t)|socialis(m|t)|antisemit(e|ic|ism)|anti-semite|anti-semitism|semite|fur(ry|sona)|sona|babyfur|diaper|ageregression|(neo)?[-]?nazi|elon|musk|war crime|whataboutism|GOP|(anti)?[-]?(vaccine|vax|vaxx|vaxxed)|vaccination|covid|coronavirus|pandemic|immunization|(?-i:ICE)|congress(men|women|ional)?|secretary of defense|(sco|po)tus|FBI)s?\b")
                .build()
                .unwrap(),
            db: db.clone(),
            pending_posts:pending_posts.clone(),
            pending_likes:pending_likes.clone(),
        },
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
            cleanup_posts(&db_clone, MY_FEED, 50_000).await;
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
        flush_task
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
    my_regex: Regex,
    db: Arc<Mutex<Connection>>,
    pending_posts: Arc<Mutex<Vec<(String, String, i64, String)>>>,
    pending_likes: Arc<Mutex<Vec<(String, String)>>>,
}

impl FeedHandler for MyFeedHandler {
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

    async fn like_post(&mut self, like_uri: Uri, liked_post_uri: Uri) {
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

    async fn serve_feed(&self, request: Request) -> FeedResult {
        info!("Serving {request:?}");

        let start_index = request
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);

        let posts_per_page = 50;
        let threshold = if request.feed == FR_FEED {
            0.05
        } else if request.feed == MY_FEED {
            0.005
        } else {
            error!("Requested a nonexistent feed");
            return FeedResult {
                cursor: None,
                feed: Vec::new(),
            };
        };

        let db = self.db.lock().await;
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
                  WHERE posts.feed = ?4
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
                WHERE rank <= ?1
                ORDER BY timestamp DESC;
                LIMIT ?2 OFFSET ?3
                ",
            )
            .expect("Failed to prepare statement");

        let post_iter = stmt
            .query_map(
                params![
                    threshold,
                    posts_per_page as i64,
                    start_index as i64,
                    request.feed
                ],
                |row| row.get::<_, String>(0),
            )
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
    let cleaned_posts = db
        .lock()
        .await
        .execute(
            "
                DELETE FROM posts
                WHERE uri NOT IN (
                    SELECT uri
                    FROM posts
                    WHERE feed = ?1
                    ORDER BY timestamp DESC
                    LIMIT ?2
                );
            ",
            params![feed, post_limit],
        )
        .expect("Failed to clean up old posts");

    trace!("Cleaned up {cleaned_posts} posts on {feed}");
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
