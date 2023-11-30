use async_stream::try_stream;
use futures::{pin_mut, StreamExt};
use futures::{stream::FuturesUnordered, Stream};
use librespot::protocol::keyexchange::ProductFlags;
use librespot::{
	audio::{AudioDecrypt, AudioFile},
	core::{
		cache::Cache,
		config::SessionConfig,
		session::Session,
		spotify_id::{FileId, SpotifyId},
	},
	discovery::Credentials,
	metadata::{FileFormat, Metadata, Track},
};

use std::io::ErrorKind;
use std::pin::{self, Pin};
use std::{fs::File, future};
use std::{
	io::{Read, Seek, SeekFrom, Write},
	path::Path,
};

use crate::{convert::Converter, format::Strategy};
use crate::{error::DownOnSpotError, format::is_ogg};

pub struct DownloadClient {
	session: Session,
	download_progress_queue:
		Vec<Pin<Box<dyn Stream<Item = Result<DownloadProgress, DownOnSpotError>>>>>,
}

pub struct DecryptedAudioFile {
	pub is_ogg: bool,
	pub audio_decrypt: AudioDecrypt<AudioFile>,
	pub size: usize,
	pub format: FileFormat,
}

pub enum DownloadProgress {
	Started,
	Progress(usize, usize), // Current, Total
	Finished,
}

pub const SPOTIFY_OGG_HEADER_END: u64 = 0xA7;

impl DownloadClient {
	pub async fn new(credentials: Credentials) -> Result<DownloadClient, DownOnSpotError> {
		let config = SessionConfig::default();
		let credentials_cache = Path::new("credentials_cache");
		let cache = Cache::new(credentials_cache.into(), None, None, None).unwrap();
		let (session, _) = Session::connect(config, credentials, cache.into(), true).await?;

		log::info!("Connected to Spotify");

		Ok(Self {
			session,
			download_progress_queue: vec![],
		})
	}

	/// Retain unfinished downloads from the download progress queue.
	pub async fn retain_unfinished(&mut self) {
		let mut new_download_progress_queue = Vec::new();

		// Filter out every download that is finished.
		while let Some(mut download) = self.download_progress_queue.pop() {
			if let Some(Ok(progress)) = download.next().await {
				if let DownloadProgress::Finished = progress {
					continue;
				}

				new_download_progress_queue.push(download);
			}
		}

		self.download_progress_queue = new_download_progress_queue;
	}

	/// Get track from id.
	async fn get_track(&self, id: SpotifyId) -> Result<Track, DownOnSpotError> {
		self.find_available_track(id)
			.await
			.ok_or_else(|| DownOnSpotError::Unavailable)
	}

	/// Get file id for given track and strategy.
	async fn get_file_id(
		&self,
		strategy: Strategy,
		track: Track,
	) -> Result<(FileId, FileFormat), DownOnSpotError> {
		let formats = strategy.formats();

		formats
			.iter() // Ordered by format.
			.find_map(|format| {
				let file_id = track.files.get(format)?;

				Some((*file_id, *format))
			})
			.ok_or_else(|| DownOnSpotError::Unavailable)
	}

	async fn decrypt(
		&self,
		strategy: Strategy,
		track: Track,
	) -> Result<DecryptedAudioFile, DownOnSpotError> {
		let id = track.id;
		let (file_id, format) = self.get_file_id(strategy, track).await?;

		let audio_file = AudioFile::open(&self.session, file_id, 1024 * 1024 * 1024, true).await?;
		let size = audio_file.get_stream_loader_controller().len();
		let key = self.session.audio_key().request(id, file_id).await?;

		// Decrypt audio file.
		let mut audio_decrypt = AudioDecrypt::new(key.into(), audio_file);

		// OGG files have a header that needs to be skipped.
		let is_ogg = is_ogg(format);
		let offset = if is_ogg {
			audio_decrypt.seek(SeekFrom::Start(SPOTIFY_OGG_HEADER_END))?; // The header is irrelevant.

			SPOTIFY_OGG_HEADER_END
		} else {
			0
		} as usize;

		Ok(DecryptedAudioFile {
			is_ogg,
			audio_decrypt,
			size: size - offset,
			format,
		})
	}

	/// Get reader for given track and strategy.
	/// If mp3 is true, convert OGG to MP3.
	async fn reader(
		&self,
		id: SpotifyId,
		strategy: Strategy,
		mp3: bool,
	) -> Result<(usize, Box<dyn Read>), DownOnSpotError> {
		let track = self.get_track(id).await?;

		let decrypted = self.decrypt(strategy, track).await?;

		let reader: Box<dyn Read> = if decrypted.is_ogg && mp3 {
			let converter = Converter::new(decrypted.audio_decrypt, decrypted.format.into())?;

			Box::new(converter)
		} else {
			Box::new(decrypted.audio_decrypt)
		};

		Ok((decrypted.size, reader))
	}

	pub fn download<'a>(
		&'a self,
		id: SpotifyId,
		strategy: Strategy,
		output: &'a str,
		mp3: bool,
	) -> impl Stream<Item = Result<DownloadProgress, DownOnSpotError>> + 'a {
		try_stream! {
			yield DownloadProgress::Started;
			let (size, mut reader) = self.reader(id, strategy, mp3).await?;

			let mut file: Vec<u8> = vec![];

			let mut current = 0;
			loop {
				let mut buffer = [0; 1024 * 64];

				match reader.read(&mut buffer) {
					Ok(0) => {
						yield DownloadProgress::Finished;
					break;
					}
					Ok(bytes_read) => {
						file.extend_from_slice(&buffer[..bytes_read]);

						current += bytes_read;
						yield DownloadProgress::Progress(current, size);
					}
					Err(e) => {
						if e.kind() == ErrorKind::Interrupted {
							continue;
						}

						return;
					}
				}
			}

			// Write audio file.
			File::create(output)?.write_all(&file)?;

		}
	}

	/// Find available track.
	/// If not found, fallback to alternative tracks.
	async fn find_available_track(&self, spotify_id: SpotifyId) -> Option<Track> {
		let track = Track::get(&self.session, spotify_id).await.ok()?;

		if !track.files.is_empty() {
			return Some(track);
		}

		let alternative = track
			.alternatives
			.iter()
			.map(|alt_id| Track::get(&self.session, *alt_id))
			.collect::<FuturesUnordered<_>>()
			.filter_map(|x| future::ready(x.ok()))
			.filter(|x| future::ready(x.available))
			.next()
			.await;

		alternative
	}
}
