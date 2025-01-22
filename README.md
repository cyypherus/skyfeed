# Skyfeed

![rust](https://github.com/cyypherus/skyfeed/actions/workflows/rust.yml/badge.svg)
[![crates.io](https://img.shields.io/crates/v/skyfeed.svg)](https://crates.io/crates/skyfeed)
[![downloads](https://img.shields.io/crates/d/skyfeed.svg)](https://crates.io/crates/skyfeed)
[![license](https://img.shields.io/crates/l/skyfeed.svg)](https://github.com/cyypherus/skyfeed/blob/main/LICENSE)

A library for quickly building bluesky feed generators.

Primarily uses, [warp](https://github.com/seanmonstar/warp), [atrium api](https://github.com/sugyan/atrium), and [jetstream-oxide](https://github.com/videah/jetstream-oxide) to greatly simplify the process of building a bluesky feed generator.

# Quick Start

Create a .env file with the following variables:

<details>
    <summary>PUBLISHER_DID</summary>

Your DID.

This can be a little hard to track down - you can use [this utility](./src/bin/my_did.rs) to check your DID

To run the my_did utility - clone this repo & run this command inside the crate directory
`cargo run --bin my_did --handle <your handle> --app-password <app password>`

```
PUBLISHER_DID="..."
```

</details>

<details>
    <summary>FEED_GENERATOR_HOSTNAME</summary>

The host name for your feed generator.
(In the URL `https://github.com/cyypherus/skyfeed` the host name is `github.com`)

You can develop your feed locally without setting this to a real value. However, when publishing, this value must be a domain that:

- Points to your service.
- Is secured with SSL (HTTPS).
- Is accessible on the public internet.

```
FEED_GENERATOR_HOSTNAME="..."

```

</details>

# Building a Feed

Let's build a simple feed generator about cats.

**Note** In a real implementation storage should be implemented with a database such as sqlite for more efficient queries & persistent data.
See the [sqlite example](./examples/sqlite)

## Implement the `FeedHandler` Trait

Your feed handler is responsible for storing and managing firehose input. For the sake of simplicity, we'll just use Vec and HashMap to manage posts and likes.

```rust
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

```

## Implement the `Feed` trait

We'll need to use `Arc<Mutex<FeedHandler>>` to enable concurrent shared access.

```rust
struct MyFeed {
    handler: MyFeedHandler,
}

impl Feed<MyFeedHandler> for MyFeed {
    fn handler(&mut self) -> MyFeedHandler {
        self.handler.clone()
    }
}
```

## Start your feed!

Now we can create an instance of our `Feed` and start it on a local address.

```rust
#[tokio::main]
async fn main() {
    let mut feed = MyFeed {
        handler: MyFeedHandler {
            posts: Arc::new(Mutex::new(Vec::new())),
        },
    };
    feed.start("Cats", ([0, 0, 0, 0], 3030)).await
}
```

## Publish to BlueSky

This repo also contains [publish](./src/bin/publish.rs) (and [unpublish](./src/bin/unpublish.rs)) utilities for managing your feed's publicity.

To run these, clone this repo & run this command inside the crate directory
`cargo run --bin publish`

If you'd like to verify your feed server's endpoints _locally_ before you publish, you can also use the [verify](./src/bin/verify.rs) utility.
