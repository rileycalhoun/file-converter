use std::{env, net::SocketAddr};

use anyhow::{Context, Result};

pub struct Config {
    pub bind_address: SocketAddr,
    pub gateway_token: String,
    pub cloudconvert_api_key: String,
    pub cloudconvert_api_base: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_address = env::var("BIND_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()
            .context("BIND_ADDRESS must be a socket address such as 127.0.0.1:8080")?;
        let gateway_token = required("GATEWAY_TOKEN")?;
        let cloudconvert_api_key = required("CLOUDCONVERT_API_KEY")?;
        let cloudconvert_api_base = env::var("CLOUDCONVERT_API_BASE")
            .unwrap_or_else(|_| "https://api.cloudconvert.com".into())
            .trim_end_matches('/')
            .to_string();

        Ok(Self {
            bind_address,
            gateway_token,
            cloudconvert_api_key,
            cloudconvert_api_base,
        })
    }
}

fn required(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("{name} must be set"))?;
    if value.trim().is_empty() {
        anyhow::bail!("{name} cannot be empty");
    }
    Ok(value)
}
