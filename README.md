# Skyfeed

[![crates.io](https://img.shields.io/crates/v/skyfeed.svg)](https://crates.io/crates/skyfeed)
[![downloads](https://img.shields.io/crates/d/skyfeed.svg)](https://crates.io/crates/skyfeed)
[![license](https://img.shields.io/crates/l/skyfeed.svg)](https://github.com/ejjonny/skyfeed/blob/main/LICENSE)

A library for quickly building bluesky feed generators.

Primarily uses, [warp](https://github.com/seanmonstar/warp), [atrium api](https://github.com/sugyan/atrium), and [jetstream-oxide](https://github.com/videah/jetstream-oxide) to greatly simplify the process of building a bluesky feed generator.

# Quick Start

Create a .env file with the following variables:

<details>
    <summary>PUBLISHER_BLUESKY_HANDLE</summary>

Your handle - something like "someguy.bsky.social"

```
PUBLISHER_BLUESKY_HANDLE="someguy.bsky.social"
```

</details>

<details>
    <summary>PUBLISHER_BLUESKY_PASSWORD</summary>

An app password. You can create app passwords [here](https://bsky.app/settings/app-passwords)

```
PUBLISHER_BLUESKY_PASSWORD="..."
```

</details>

<details>
    <summary>PUBLISHER_DID</summary>

Your DID.

This can be a little hard to track down - you can use [this utility](./src/bin/my_did.rs) to check your DID once you've added `PUBLISHER_BLUESKY_HANDLE` & `PUBLISHER_BLUESKY_PASSWORD` to your .env file.

To run the my_did utility - clone this repo & run this command inside the crate directory
`cargo run --bin my_did`

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

[Note]
In a real implementation storage should be implemented with a database such as sqlite for more efficient queries & persistent data.
This example also doesn't handle pagination - the `Cursor` that is part of the `serve_feed` input & output. A real implementation should use the cursor to only serve posts the user hasn't seen yet.

## Implement the `FeedHandler` Trait

Your feed handler is responsible for storing and managing firehose input. For the sake of simplicity, we'll use HashMaps to manage posts and likes.

```rust
struct MyFeedHandler {
    posts: HashMap<Uri, Post>,
    likes: HashMap<Uri, Uri>,
}

impl FeedHandler for MyFeedHandler {
    async fn insert_post(&mut self, post: Post) {
        if post.text.to_lowercase().contains(" cat ") {
            info!("Storing {post:?}");
            const MAX_POSTS: usize = 100;

            self.posts.insert(post.uri.clone(), post);

            if self.posts.len() > MAX_POSTS {
                let mut post_likes: HashMap<&Uri, u32> = HashMap::new();

                for liked_post_uri in self.likes.values() {
                    *post_likes.entry(liked_post_uri).or_insert(0) += 1;
                }
                if let Some(least_liked_uri) = self
                    .posts
                    .keys()
                    .min_by_key(|uri| post_likes.get(uri).unwrap_or(&0))
                    .cloned()
                {
                    self.posts.remove(&least_liked_uri);
                }
            }
        }
    }

    async fn delete_post(&mut self, uri: Uri) {
        self.posts.remove(&uri);
    }

    async fn like_post(&mut self, like_uri: Uri, liked_post_uri: Uri) {
        self.likes.insert(like_uri, liked_post_uri);
    }

    async fn delete_like(&mut self, like_uri: Uri) {
        self.likes.remove(&like_uri);
    }

    async fn serve_feed(&self, request: Request) -> FeedResult {
        info!("Serving {request:?}");
        let mut post_likes: HashMap<&Uri, u32> = HashMap::new();
        for liked_post_uri in self.likes.values() {
            *post_likes.entry(liked_post_uri).or_insert(0) += 1;
        }

        let mut top_posts: Vec<_> = self.posts.values().collect();
        top_posts.sort_by(|a, b| {
            let likes_a = post_likes.get(&a.uri).unwrap_or(&0);
            let likes_b = post_likes.get(&b.uri).unwrap_or(&0);
            likes_b.cmp(likes_a)
        });

        let top_5_posts: Vec<_> = top_posts.into_iter().take(5).collect();

        FeedResult {
            cursor: None,
            feed: top_5_posts
                .into_iter()
                .map(|post| post.uri.clone())
                .collect(),
        }
    }
}
```

## Implement the `Feed` trait

We'll need to use `Arc<Mutex<FeedHandler>>` to enable concurrent shared access.

```rust
struct MyFeed {
    handler: Arc<Mutex<MyFeedHandler>>,
}

impl Feed<MyFeedHandler> for MyFeed {
    fn handler(&mut self) -> Arc<Mutex<MyFeedHandler>> {
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
        handler: Arc::new(Mutex::new(MyFeedHandler {
            posts: HashMap::new(),
            likes: HashMap::new(),
        })),
    };
    feed.start("Cats", ([0, 0, 0, 0], 3030)).await
}
```

## Publish to bluesky

This repo also contains [publish](./src/bin/publish.rs) (and [unpublish](./src/bin/unpublish.rs)) utilities for managing your feed's publicity.

To run these, clone this repo & run this command inside the crate directory
`cargo run --bin publish`

If you'd like to verify your feed server's endpoints _locally_ before you publish, you can also use the [verify](./src/bin/verify.rs) utility.
