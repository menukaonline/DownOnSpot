use std::path::Path;

use crate::error::DownOnSpotError;

use rspotify::{
	model::{
		AlbumId, ArtistId, FullAlbum, FullArtist, FullPlaylist, FullTrack, PlaylistId, TrackId,
	},
	prelude::BaseClient,
	ClientCredsSpotify, Credentials,
};
use url::Url;

pub struct MetadataClient {
	spotify: ClientCredsSpotify,
}

pub enum SpotifyItem {
	Track(FullTrack),
	Album(FullAlbum),
	Playlist(FullPlaylist),
	Artist(FullArtist),
}

impl MetadataClient {
	pub async fn new(credentials: Credentials) -> Result<MetadataClient, DownOnSpotError> {
		let spotify = ClientCredsSpotify::with_config(
			credentials,
			rspotify::Config {
				token_cached: true,
				token_refreshing: true,
				cache_path: Path::new("credentials_cache")
					.join(rspotify::DEFAULT_CACHE_PATH)
					.into(),
				..Default::default()
			},
		);

		spotify.request_token().await?;

		Ok(Self { spotify })
	}

	/// Get Spotify item from URL.
	async fn from_url(&self, input: &str) -> Result<SpotifyItem, DownOnSpotError> {
		let url = Url::parse(input)?;

		let invalid_uri_error = || DownOnSpotError::Invalid("Invalid Spotify URL".to_owned());
		let domain = url.domain().ok_or_else(invalid_uri_error)?;

		if !domain.to_lowercase().ends_with("spotify.com") {
			return Err(invalid_uri_error());
		}

		let mut segments = url.path_segments().ok_or_else(invalid_uri_error)?;
		let item_type = segments
			.next()
			.ok_or_else(invalid_uri_error)?
			.replace("/", "");
		let spotify_id = segments.next_back().ok_or_else(invalid_uri_error)?;

		self.from_id(&item_type, spotify_id).await
	}

	/// Get Spotify item from ID.
	async fn from_id(
		&self,
		item_type: &str,
		spotify_id: &str,
	) -> Result<SpotifyItem, DownOnSpotError> {
		let spotify_item = match item_type {
			"track" => {
				let track_id = TrackId::from_id_or_uri(spotify_id)?;
				let full_track = self.spotify.track(track_id).await?;

				SpotifyItem::Track(full_track)
			}
			"album" => {
				let album_id = AlbumId::from_id_or_uri(spotify_id)?;
				let full_album = self.spotify.album(album_id).await?;

				SpotifyItem::Album(full_album)
			}
			"playlist" => {
				let playlist_id = PlaylistId::from_id_or_uri(spotify_id)?;
				let full_playlist = self.spotify.playlist(playlist_id, None, None).await?;

				SpotifyItem::Playlist(full_playlist)
			}
			"artist" => {
				let artist_id = ArtistId::from_id_or_uri(spotify_id)?;
				let full_artist = self.spotify.artist(artist_id).await?;

				SpotifyItem::Artist(full_artist)
			}
			_ => return Err(DownOnSpotError::InvalidId),
		};

		Ok(spotify_item)
	}

	pub async fn parse(&self, input: &str) -> Result<SpotifyItem, DownOnSpotError> {
		let item = self.from_url(input).await;

		// Try parsing as URL.
		if item.is_ok() {
			return item;
		}

		// Try parsing as Spotify ID.
		let invalid_id = || DownOnSpotError::Invalid("Invalid Spotify URL or ID".to_string());

		let mut splits = input
			.strip_prefix("spotify:")
			.ok_or_else(invalid_id)?
			.split(":");

		let item_type = splits.next().ok_or_else(invalid_id)?;
		let spotify_id = splits.next().ok_or_else(invalid_id)?;

		self.from_id(item_type, spotify_id).await
	}
}
