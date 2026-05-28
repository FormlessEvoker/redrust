use std::env;
use std::error::Error;

pub struct Config {
    pub port: u16,
}

pub fn load_env() -> Result<Config, Box<dyn Error>> {
    // First, load .env file
    dotenvy::dotenv().ok();

    let port = load_port()?;

    Ok(Config { port })
}

fn load_port() -> Result<u16, Box<dyn Error>> {
    let port_str = env::var("PORT")?; // error if missing
    let port = port_str.parse::<u16>()?; // error if invalid

    Ok(port)
}
