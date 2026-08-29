use std::{env, net::SocketAddr};

use anyhow::{Context, Result};

pub struct Config {
    pub bind_address: SocketAddr,
    pub gateway_token: String,
    pub gotenberg_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_address = env::var("BIND_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()
            .context("BIND_ADDRESS must be a socket address such as 127.0.0.1:8080")?;
        let gateway_token = required("GATEWAY_TOKEN")?;
        let gotenberg_url = env::var("GOTENBERG_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3000".into())
            .trim_end_matches('/')
            .to_string();
        let parsed = reqwest::Url::parse(&gotenberg_url)
            .context("GOTENBERG_URL must be a complete HTTP or HTTPS URL")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            anyhow::bail!("GOTENBERG_URL must be a complete HTTP or HTTPS URL");
        }

        Ok(Self {
            bind_address,
            gateway_token,
            gotenberg_url,
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
