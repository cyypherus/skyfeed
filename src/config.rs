use std::env;

use dotenv::dotenv;

#[derive(Debug, Clone)]
pub struct Config {
    pub feed_generator_did: String,
    pub publisher_did: String,
    pub feed_generator_hostname: String,
}

impl Config {
    pub fn load_env_config() -> Self {
        dotenv().unwrap();
        Config {
            feed_generator_did: env::var("FEED_GENERATOR_DID").unwrap(),
            publisher_did: env::var("PUBLISHER_DID").unwrap(),
            feed_generator_hostname: env::var("FEED_GENERATOR_HOSTNAME").unwrap(),
        }
    }
}
