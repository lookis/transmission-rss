use anyhow::{Context, Result};
use clap::Parser;
use quick_xml::events::Event;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use transmission_rpc::{TransClient, types::BasicAuth, types::TorrentAddArgs};
use url::Url;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the app configuration file
    #[arg(short, long, default_value = "config/app.yaml")]
    config: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(rename = "transmission-rpc")]
    transmission_rpc: TransmissionConfig,
    rss: Vec<RssConfig>,
    parser: HashMap<String, ParserConfig>,
}

#[derive(Debug, Deserialize)]
struct TransmissionConfig {
    host: String,
    port: u16,
    path: String,
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct RssConfig {
    url: String,
    parser: String,
}

#[derive(Debug, Deserialize)]
struct ParserConfig {
    path: String,
    property: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Read app config
    let config_content = fs::read_to_string(&args.config).context("Failed to read app config")?;
    let config: Config =
        serde_yaml::from_str(&config_content).context("Failed to parse app config")?;

    // Create transmission client
    let mut client = TransClient::with_auth(
        Url::parse(&format!(
            "http://{}:{}/{}",
            config.transmission_rpc.host,
            config.transmission_rpc.port,
            config.transmission_rpc.path
        ))?,
        BasicAuth {
            user: config.transmission_rpc.username,
            password: config.transmission_rpc.password,
        },
    );

    // Process each RSS feed
    for rss_config in config.rss {
        // Get parser config for this RSS feed
        let parser_config = config.parser.get(&rss_config.parser).with_context(|| {
            format!("Parser '{}' not found in configuration", rss_config.parser)
        })?;

        // Download RSS feed
        let response = reqwest::get(&rss_config.url)
            .await
            .with_context(|| format!("Failed to download RSS feed: {}", rss_config.url))?;
        let xml_content = response
            .text()
            .await
            .with_context(|| format!("Failed to get RSS content: {}", rss_config.url))?;

        // Parse XML and extract torrent URLs
        let urls = parse_xml(&xml_content, parser_config)?;

        // Add torrents to transmission
        for url in urls {
            println!("Adding torrent: {}", url);
            let args = TorrentAddArgs {
                filename: Some(url.clone()),
                ..Default::default()
            };
            if let Err(e) = client.torrent_add(args).await {
                eprintln!("Failed to add torrent {}: {}", url, e);
            }
        }
    }

    Ok(())
}

fn parse_xml(xml_content: &str, parser_config: &ParserConfig) -> Result<Vec<String>> {
    let mut urls = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml_content);
    reader.config_mut().trim_text(true);

    // Parse the path configuration
    let path_parts: Vec<&str> = parser_config.path.split(',').collect();
    let mut current_path = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf).unwrap() {
            Event::Start(e) => {
                let name = std::str::from_utf8(e.name().into_inner())?;
                current_path.push(name.to_string());
            }
            Event::End(_) => {
                current_path.pop();
            }
            Event::Empty(e) => {
                let name = std::str::from_utf8(e.name().into_inner())?;
                let mut check_path = current_path.clone();
                check_path.push(name.to_string());
                if check_path == path_parts {
                    let attributes = e.attributes();
                    for attr in attributes {
                        if let Ok(attr) = attr {
                            if let Ok(key) = std::str::from_utf8(attr.key.into_inner()) {
                                if key == parser_config.property {
                                    if let Ok(value) = std::str::from_utf8(&attr.value.into_owned())
                                    {
                                        urls.push(value.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => (),
        }
        buf.clear();
    }
    Ok(urls)
}
