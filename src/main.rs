use std::env::var;

use args::Args;
use clap::Parser;
use dotenv::dotenv;
use download::DownloadClient;
use error::DownOnSpotError;
use futures::{pin_mut, StreamExt};
use librespot::core::spotify_id::SpotifyId;
use librespot::discovery::Credentials;
use simple_logger::SimpleLogger;
use spotify::MetadataClient;

use crate::spotify::SpotifyItem;
mod args;
mod convert;
mod download;
mod error;
mod format;
mod spotify;

fn setup_logging() -> Result<(), DownOnSpotError> {
	SimpleLogger::new()
		.with_level(log::LevelFilter::Off)
		.with_module_level("down_on_spot", log::LevelFilter::Debug)
		.init()
		.map_err(|e| DownOnSpotError::Error(e.to_string()))
}

fn setup_env() -> Result<(), DownOnSpotError> {
	dotenv().map_err(|e| DownOnSpotError::Error(e.to_string()))?;
	Ok(())
}

#[tokio::main]
async fn main() {
	if let Err(error) = run().await {
		log::error!("{}", error);
	}
}

async fn run() -> Result<(), DownOnSpotError> {
	setup_logging()?;
	setup_env()?;

	let args = Args::parse();

	let (metadata_client, download_client) = futures::join!(
		MetadataClient::new(rspotify::Credentials {
			id: var("SPOTIFY_CLIENT_ID")
				.expect("SPOTIFY_CLIENT_ID must be set.")
				.into(),
			secret: var("SPOTIFY_CLIENT_SECRET")
				.expect("SPOTIFY_CLIENT_SECRET must be set.")
				.into(),
		}),
		DownloadClient::new(Credentials::with_password(
			var("SPOTIFY_USERNAME").expect("SPOTIFY_USERNAME must be set."),
			var("SPOTIFY_PASSWORD").expect("SPOTIFY_PASSWORD must be set."),
		))
	);
	let (download_client, metadata_client) = (download_client?, metadata_client?);

	let item = metadata_client.parse(&args.input).await?;

	// TODO: Handle other types of items.
	// TODO: Parallelize downloads.
	if let SpotifyItem::Track(track) = item {
		let id = &track.id.unwrap().to_string();
		let id = SpotifyId::from_uri(id)?;

		let download = download_client.download(id, args.strategy, &args.output, args.mp3);

		pin_mut!(download);

		while let Some(progress) = download.next().await {
			print!("{esc}[2J{esc}[1;1H", esc = 27 as char);

			match progress? {
				download::DownloadProgress::Started => {
					log::info!("Started download");
				}
				download::DownloadProgress::Finished => {
					log::info!("Finished download");
				}
				download::DownloadProgress::Progress(current, size) => {
					log::info!(
						"Download progress: {:.2}%",
						(current as f64 / size as f64) * 100.0
					);
				}
			}
		}
	}

	Ok(())
}
