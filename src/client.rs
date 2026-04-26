use std::time::Duration;

use reqwest::header;
use serde::Deserialize;

// -- Response types (model only the fields the exporter needs) --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Session {
    pub user_name: Option<String>,
    pub client: Option<String>,
    pub device_name: Option<String>,
    pub now_playing_item: Option<NowPlayingItem>,
    pub play_state: Option<PlayState>,
    pub transcoding_info: Option<TranscodingInfo>,
    /// IPv4/IPv6 address the client is connecting from. Exposed as a metric
    /// label only when the operator opts in via `EXPOSE_REMOTE_ADDRESS=true`.
    pub remote_end_point: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NowPlayingItem {
    pub name: Option<String>,
    #[serde(rename = "Type")]
    pub media_type: Option<String>,
    pub bitrate: Option<u64>,
    pub media_streams: Option<Vec<MediaStream>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    pub bit_rate: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayState {
    pub play_method: Option<String>,
    pub is_paused: Option<bool>,
    /// Playback position in Jellyfin ticks (10,000,000 ticks per second).
    pub position_ticks: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TranscodingInfo {
    pub bitrate: Option<u64>,
    pub completion_percentage: Option<f64>,
    pub hardware_acceleration_type: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub transcode_reasons: Option<Vec<String>>,
    pub is_video_direct: Option<bool>,
    pub is_audio_direct: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Library {
    pub name: String,
    pub collection_type: Option<String>,
    pub item_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemCounts {
    pub movie_count: u64,
    pub series_count: u64,
    pub episode_count: u64,
    #[serde(default)]
    pub book_count: u64,
    pub song_count: u64,
    pub album_count: u64,
    #[serde(default)]
    pub artist_count: u64,
    #[serde(default)]
    pub trailer_count: u64,
    #[serde(default)]
    pub music_video_count: u64,
    #[serde(default)]
    pub box_set_count: u64,
    #[serde(default)]
    pub item_count: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub server_name: String,
    pub version: String,
    pub operating_system: String,
}

/// Wrapper for paginated item responses — we only need `TotalRecordCount`.
///
/// Implementation detail of [`JellyfinClient::get_library_item_count`]; not
/// part of the public surface.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ItemsResponse {
    pub(crate) total_record_count: u64,
}

// -- Error type --

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("request timed out")]
    Timeout,

    #[error("deserialization failed: {0}")]
    Deserialization(String),
}

// -- Trait --

#[async_trait::async_trait]
pub trait JellyfinApi: Send + Sync {
    async fn get_sessions(&self) -> Result<Vec<Session>, ClientError>;
    async fn get_libraries(&self) -> Result<Vec<Library>, ClientError>;
    async fn get_item_counts(&self) -> Result<ItemCounts, ClientError>;
    async fn get_system_info(&self) -> Result<SystemInfo, ClientError>;
    async fn get_library_item_count(&self, parent_id: &str) -> Result<u64, ClientError>;

    /// Probe the unauthenticated `/System/Info/Public` endpoint.
    ///
    /// Returns `true` only when the server responds successfully to an
    /// unauthenticated request — this is a network-level liveness check,
    /// not an authentication check. A Jellyfin instance with a rotated or
    /// invalid API key will still report `true` here; the breaker on the
    /// authenticated endpoints is what catches that case.
    async fn is_publicly_reachable(&self) -> bool;
}

// -- Implementation --

pub struct JellyfinClient {
    http: reqwest::Client,
    base_url: String,
}

impl JellyfinClient {
    /// Construct a new HTTP client for a Jellyfin instance.
    ///
    /// Returns `Self` rather than `Arc<Self>` — wrapping in `Arc` is the
    /// caller's decision (the binary needs an `Arc<dyn JellyfinApi>` for
    /// the collector; tests typically don't).
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Deserialization`] if `api_key` contains
    /// characters that cannot appear in an HTTP header value.
    ///
    /// Returns [`ClientError::Http`] if the underlying `reqwest::Client`
    /// builder fails (typically a TLS-backend initialisation problem).
    pub fn new(base_url: &str, api_key: &str, timeout: Duration) -> Result<Self, ClientError> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "X-Emby-Token",
            header::HeaderValue::from_str(api_key).map_err(|e| {
                ClientError::Deserialization(format!("invalid API key header: {e}"))
            })?,
        );

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers(headers)
            .build()?;

        Ok(Self {
            http,
            base_url: base_url.to_owned(),
        })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let url = format!("{}{path}", self.base_url);

        let response = self.http.get(&url).send().await.map_err(|e| {
            if e.is_timeout() {
                ClientError::Timeout
            } else {
                ClientError::Http(e)
            }
        })?;

        let response = response.error_for_status()?;

        response.json::<T>().await.map_err(|e| {
            ClientError::Deserialization(format!("failed to parse response from {path}: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl JellyfinApi for JellyfinClient {
    async fn get_sessions(&self) -> Result<Vec<Session>, ClientError> {
        self.get("/Sessions").await
    }

    async fn get_libraries(&self) -> Result<Vec<Library>, ClientError> {
        self.get("/Library/VirtualFolders").await
    }

    async fn get_item_counts(&self) -> Result<ItemCounts, ClientError> {
        self.get("/Items/Counts").await
    }

    async fn get_system_info(&self) -> Result<SystemInfo, ClientError> {
        self.get("/System/Info").await
    }

    async fn get_library_item_count(&self, parent_id: &str) -> Result<u64, ClientError> {
        let path =
            format!("/Items?ParentId={parent_id}&Recursive=true&Limit=0&Fields=BasicSyncInfo");
        let response: ItemsResponse = self.get(&path).await?;
        Ok(response.total_record_count)
    }

    async fn is_publicly_reachable(&self) -> bool {
        // Just need a successful HTTP response; the body is unused.
        self.get::<serde::de::IgnoredAny>("/System/Info/Public")
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_session_with_now_playing() {
        let json = r#"{
            "Id": "abc123",
            "UserName": "alice",
            "Client": "Infuse",
            "DeviceName": "Apple TV",
            "NowPlayingItem": {
                "Name": "Movie Title",
                "Type": "Movie",
                "Bitrate": 20000000,
                "MediaStreams": [
                    {"Type": "Video", "Codec": "hevc", "BitRate": 18000000},
                    {"Type": "Audio", "Codec": "aac", "BitRate": 2000000}
                ]
            },
            "PlayState": {
                "PlayMethod": "Transcode",
                "IsPaused": false
            },
            "TranscodingInfo": {
                "IsVideoDirect": false,
                "VideoCodec": "h264",
                "AudioCodec": "aac",
                "Bitrate": 15000000,
                "CompletionPercentage": 45.5,
                "HardwareAccelerationType": "vaapi",
                "TranscodeReasons": ["ContainerBitrateExceedsLimit"]
            }
        }"#;

        let session: Session = serde_json::from_str(json).unwrap();
        assert_eq!(session.user_name.as_deref(), Some("alice"));
        assert_eq!(session.client.as_deref(), Some("Infuse"));
        assert_eq!(session.device_name.as_deref(), Some("Apple TV"));

        let item = session.now_playing_item.unwrap();
        assert_eq!(item.name.as_deref(), Some("Movie Title"));
        assert_eq!(item.media_type.as_deref(), Some("Movie"));
        assert_eq!(item.bitrate, Some(20_000_000));
        assert_eq!(item.media_streams.as_ref().unwrap().len(), 2);
        assert_eq!(
            item.media_streams.as_ref().unwrap()[0].bit_rate,
            Some(18_000_000)
        );

        let play_state = session.play_state.unwrap();
        assert_eq!(play_state.play_method.as_deref(), Some("Transcode"));
        assert_eq!(play_state.is_paused, Some(false));

        let transcoding = session.transcoding_info.unwrap();
        assert_eq!(transcoding.bitrate, Some(15_000_000));
        assert_eq!(transcoding.completion_percentage, Some(45.5));
        assert_eq!(
            transcoding.hardware_acceleration_type.as_deref(),
            Some("vaapi")
        );
        assert_eq!(transcoding.video_codec.as_deref(), Some("h264"));
        assert_eq!(transcoding.audio_codec.as_deref(), Some("aac"));
        assert_eq!(
            transcoding.transcode_reasons.as_deref(),
            Some(&["ContainerBitrateExceedsLimit".to_owned()][..])
        );
        assert_eq!(transcoding.is_video_direct, Some(false));
        assert_eq!(transcoding.is_audio_direct, None);
    }

    #[test]
    fn deserialize_session_idle() {
        let json = r#"{
            "Id": "idle1",
            "UserName": "guest",
            "Client": "Web",
            "DeviceName": "Chrome",
            "NowPlayingItem": null,
            "PlayState": null,
            "TranscodingInfo": null
        }"#;

        let session: Session = serde_json::from_str(json).unwrap();
        assert!(session.now_playing_item.is_none());
        assert!(session.play_state.is_none());
        assert!(session.transcoding_info.is_none());
    }

    #[test]
    fn deserialize_session_missing_optional_fields() {
        // Jellyfin may omit fields entirely (not just null them)
        let json = r#"{
            "Id": "minimal"
        }"#;

        let session: Session = serde_json::from_str(json).unwrap();
        assert!(session.user_name.is_none());
        assert!(session.client.is_none());
        assert!(session.now_playing_item.is_none());
    }

    #[test]
    fn deserialize_library() {
        let json = r#"{
            "Name": "Movies",
            "CollectionType": "movies",
            "ItemId": "abc-123"
        }"#;

        let lib: Library = serde_json::from_str(json).unwrap();
        assert_eq!(lib.name, "Movies");
        assert_eq!(lib.collection_type.as_deref(), Some("movies"));
        assert_eq!(lib.item_id, "abc-123");
    }

    #[test]
    fn deserialize_library_without_collection_type() {
        let json = r#"{
            "Name": "Mixed",
            "ItemId": "xyz-789"
        }"#;

        let lib: Library = serde_json::from_str(json).unwrap();
        assert_eq!(lib.name, "Mixed");
        assert!(lib.collection_type.is_none());
    }

    #[test]
    fn deserialize_item_counts() {
        let json = r#"{
            "MovieCount": 150,
            "SeriesCount": 30,
            "EpisodeCount": 800,
            "ArtistCount": 50,
            "ProgramCount": 0,
            "TrailerCount": 0,
            "SongCount": 500,
            "AlbumCount": 40,
            "MusicVideoCount": 0,
            "BoxSetCount": 5,
            "BookCount": 20,
            "ItemCount": 1595
        }"#;

        let counts: ItemCounts = serde_json::from_str(json).unwrap();
        assert_eq!(counts.movie_count, 150);
        assert_eq!(counts.series_count, 30);
        assert_eq!(counts.episode_count, 800);
        assert_eq!(counts.book_count, 20);
        assert_eq!(counts.song_count, 500);
        assert_eq!(counts.album_count, 40);
        assert_eq!(counts.artist_count, 50);
        assert_eq!(counts.trailer_count, 0);
        assert_eq!(counts.music_video_count, 0);
        assert_eq!(counts.box_set_count, 5);
        assert_eq!(counts.item_count, 1595);
    }

    #[test]
    fn deserialize_item_counts_missing_book_defaults_to_zero() {
        let json = r#"{
            "MovieCount": 10,
            "SeriesCount": 5,
            "EpisodeCount": 50,
            "SongCount": 0,
            "AlbumCount": 0
        }"#;

        let counts: ItemCounts = serde_json::from_str(json).unwrap();
        assert_eq!(counts.book_count, 0);
    }

    #[test]
    fn deserialize_system_info() {
        let json = r#"{
            "ServerName": "jellyfin-srv",
            "Version": "10.9.11",
            "OperatingSystem": "Linux",
            "StartupWizardCompleted": true
        }"#;

        let info: SystemInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.server_name, "jellyfin-srv");
        assert_eq!(info.version, "10.9.11");
        assert_eq!(info.operating_system, "Linux");
    }

    #[test]
    fn deserialize_items_response() {
        let json = r#"{
            "Items": [],
            "TotalRecordCount": 42,
            "StartIndex": 0
        }"#;

        let resp: ItemsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.total_record_count, 42);
    }

    #[test]
    fn unexpected_shape_fails_deserialization() {
        // SystemInfo missing required field `Version`
        let json = r#"{
            "ServerName": "test",
            "OperatingSystem": "Linux"
        }"#;

        let result = serde_json::from_str::<SystemInfo>(json);
        assert!(result.is_err());
    }
}
