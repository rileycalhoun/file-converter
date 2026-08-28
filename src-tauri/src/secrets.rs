use anyhow::{Context, Result};
use keyring::{Entry, Error};

const SERVICE: &str = "us.thecalhouns.file-converter";
const GATEWAY_TOKEN_ACCOUNT: &str = "gateway-access-token";

pub struct SecretStore;

impl SecretStore {
    pub fn new() -> Self {
        Self
    }

    pub fn set_gateway_token(&self, token: &str) -> Result<()> {
        entry()?
            .set_password(token)
            .context("Could not save the gateway token in the operating system credential store.")
    }

    pub fn gateway_token(&self) -> Result<String> {
        match entry()?.get_password() {
            Ok(token) if !token.is_empty() => Ok(token),
            Ok(_) | Err(Error::NoEntry) => {
                anyhow::bail!("Configure the gateway token in Settings first.")
            }
            Err(error) => Err(error).context(
                "Could not read the gateway token from the operating system credential store.",
            ),
        }
    }

    pub fn gateway_token_configured(&self) -> Result<bool> {
        match entry()?.get_password() {
            Ok(token) => Ok(!token.is_empty()),
            Err(Error::NoEntry) => Ok(false),
            Err(error) => Err(error).context(
                "Could not read the gateway token from the operating system credential store.",
            ),
        }
    }
}

fn entry() -> Result<Entry> {
    Entry::new(SERVICE, GATEWAY_TOKEN_ACCOUNT)
        .context("The operating system credential store is unavailable.")
}
