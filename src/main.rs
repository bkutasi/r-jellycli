use alsa::Direction;
use alsa::PCM;
use alsa::pcm::{Access, Format, HwParams};
use alsa::ValueOr;
use clap::Parser;
use reqwest::Client;
use serde::Deserialize;
use std::error::Error;
use std::ffi::CString;


/// Simple Jellyfin Headless Client
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Jellyfin server URL
    #[arg(short, long)]
    server_url: String,

    /// Jellyfin API key
    #[arg(short, long)]
    api_key: String,

    /// ALSA device to use
    #[arg(short, long, default_value = "default")]
    alsa_device: String,
}

#[derive(Deserialize, Debug)]
struct MediaItem {
    id: String,
    name: String,
    media_type: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Parse command-line arguments
    let args = Args::parse();

    // Initialize HTTP client
    let client = Client::new();

    // Fetch media items from Jellyfin server
    let media_items = fetch_media_items(&client, &args.server_url, &args.api_key).await?;

    // Display media items
    println!("Available Media Items:");
    for (index, item) in media_items.iter().enumerate() {
        println!("{}: {} ({})", index, item.name, item.media_type);
    }

    // Prompt user to select a media item
    println!("Enter the number of the media item to play:");
    let mut selection = String::new();
    std::io::stdin().read_line(&mut selection)?;
    let index: usize = selection.trim().parse()?;
    if index >= media_items.len() {
        eprintln!("Invalid selection.");
        return Ok(());
    }
    let selected_item = &media_items[index];
    println!("Playing: {}", selected_item.name);

    // Here you would implement streaming and playback logic using ALSA.
    // This is a placeholder for demonstration purposes.
    play_media(&args.alsa_device)?;

    Ok(())
}

/// Fetch media items from Jellyfin server
async fn fetch_media_items(
    client: &Client,
    server_url: &str,
    api_key: &str,
) -> Result<Vec<MediaItem>, Box<dyn Error>> {0
    let url = format!("{}/Users/Default/Items", server_url);
    let resp = client
        .get(&url)
        .header("X-Emby-Token", api_key)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let items = resp["Items"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|item| MediaItem {
            id: item["Id"].as_str().unwrap_or("").to_string(),
            name: item["Name"].as_str().unwrap_or("").to_string(),
            media_type: item["Type"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    Ok(items)
}

/// Play media using the specified ALSA device
fn play_media(device: &str) -> Result<(), Box<dyn Error>> {
    let device = CString::new(device)?;
    let pcm = PCM::open(&device, Direction::Playback, false)?;
    let hwp = HwParams::any(&pcm)?;
    hwp.set_channels(2)?;
    hwp.set_rate(44100, ValueOr::Nearest)?;
    hwp.set_format(Format::s16())?;
    hwp.set_access(Access::RWInterleaved)?;
    pcm.hw_params(&hwp)?;

    let buffer = [0i16; 44100 * 2];

    println!("Playing audio on device: {}", device.to_string_lossy());
    
    // Use io_ptr() instead of writei
    let io = pcm.io_i16()?;
    for _ in 0..5 {
        io.writei(&buffer)?;
    }

    Ok(())
}
