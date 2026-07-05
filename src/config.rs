//! Environment-backed configuration for the TCP server.

use std::env;
use std::error::Error;

/// Runtime configuration loaded before the server starts.
pub struct Config {
    /// TCP port the listener binds to.
    pub port: u16,
}

/// Loads configuration from the process environment and optional `.env` file.
pub fn load_env() -> Result<Config, Box<dyn Error>> {
    // First, load .env file
    dotenvy::dotenv().ok();

    let port = load_port()?;

    Ok(Config { port })
}

/// Reads and parses the `PORT` environment variable.
fn load_port() -> Result<u16, Box<dyn Error>> {
    let port_str = env::var("PORT")?; // error if missing
    let port = port_str.parse::<u16>()?; // error if invalid

    Ok(port)
}
