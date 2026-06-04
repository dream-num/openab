use std::env::consts;
use std::env;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
#[cfg(feature = "client")]
use std::io::Read;
use std::time::SystemTime;

use base64::Engine as _;
#[cfg(feature = "client")]
use flate2::read::ZlibDecoder;
use futures::SinkExt;
#[cfg(feature = "client")]
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "client")]
use tokio::time::{timeout, Duration};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
#[cfg(feature = "client")]
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
#[cfg(feature = "client")]
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{
    client_async_tls_with_config, connect_async_with_config, MaybeTlsStream, WebSocketStream,
};
#[cfg(feature = "client")]
use tracing::warn;
use tracing::{debug, instrument, trace};
use url::Url;

use super::{ActivityData, ChunkGuildFilter, PresenceData};
use crate::constants::{self, Opcode};
#[cfg(feature = "client")]
use crate::gateway::GatewayError;
#[cfg(feature = "client")]
use crate::json::from_str;
use crate::json::to_string;
#[cfg(feature = "client")]
use crate::model::event::GatewayEvent;
use crate::model::gateway::{GatewayIntents, ShardInfo};
use crate::model::id::{GuildId, UserId};
#[cfg(feature = "client")]
use crate::Error;
use crate::Result;

#[derive(Serialize)]
struct IdentifyProperties {
    browser: &'static str,
    device: &'static str,
    os: &'static str,
}

#[derive(Serialize)]
struct ChunkGuildMessage<'a> {
    guild_id: GuildId,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<&'a str>,
    limit: u16,
    presences: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_ids: Option<Vec<UserId>>,
    nonce: &'a str,
}

#[derive(Serialize)]
struct PresenceUpdateMessage<'a> {
    afk: bool,
    status: &'a str,
    since: SystemTime,
    activities: &'a [&'a ActivityData],
}

#[derive(Serialize)]
#[serde(untagged)]
enum WebSocketMessageData<'a> {
    Heartbeat(Option<u64>),
    ChunkGuild(ChunkGuildMessage<'a>),
    SoundboardSounds {
        guild_ids: &'a [GuildId],
    },
    Identify {
        compress: bool,
        token: &'a str,
        large_threshold: u8,
        shard: &'a ShardInfo,
        intents: GatewayIntents,
        properties: IdentifyProperties,
        presence: PresenceUpdateMessage<'a>,
    },
    PresenceUpdate(PresenceUpdateMessage<'a>),
    Resume {
        session_id: &'a str,
        token: &'a str,
        seq: u64,
    },
}

#[derive(Serialize)]
struct WebSocketMessage<'a> {
    op: Opcode,
    d: WebSocketMessageData<'a>,
}

pub struct WsClient(WebSocketStream<MaybeTlsStream<TcpStream>>);

#[cfg(feature = "client")]
const TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(feature = "client")]
const DECOMPRESSION_MULTIPLIER: usize = 3;

impl WsClient {
    pub(crate) async fn connect(url: Url) -> Result<Self> {
        let config = WebSocketConfig {
            max_message_size: None,
            max_frame_size: None,
            ..Default::default()
        };
        let (stream, _) = if let Some(proxy) = proxy_from_env(&url) {
            trace!(proxy = %proxy, target = %url, "connecting to gateway via system proxy");
            let stream = connect_via_proxy(&url, &proxy).await?;
            let request = build_ws_request(&url, &proxy)?;
            client_async_tls_with_config(request, stream, Some(config), None).await?
        } else {
            connect_async_with_config(url, Some(config), false).await?
        };

        Ok(Self(stream))
    }

    #[cfg(feature = "client")]
    pub(crate) async fn recv_json(&mut self) -> Result<Option<GatewayEvent>> {
        let message = match timeout(TIMEOUT, self.0.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => return Err(e.into()),
            Ok(None) | Err(_) => return Ok(None),
        };

        let value = match message {
            Message::Binary(bytes) => {
                let mut decompressed =
                    String::with_capacity(bytes.len() * DECOMPRESSION_MULTIPLIER);

                ZlibDecoder::new(&bytes[..]).read_to_string(&mut decompressed).map_err(|why| {
                    warn!("Err decompressing bytes: {why:?}");
                    debug!("Failing bytes: {bytes:?}");

                    why
                })?;

                from_str(&decompressed).map_err(|why| {
                    warn!("Err deserializing bytes: {why:?}");
                    debug!("Failing bytes: {bytes:?}");

                    why
                })?
            },
            Message::Text(payload) => from_str(&payload).map_err(|why| {
                warn!("Err deserializing text: {why:?}; text: {payload}");

                why
            })?,
            Message::Close(Some(frame)) => {
                return Err(Error::Gateway(GatewayError::Closed(Some(frame))));
            },
            _ => return Ok(None),
        };

        Ok(Some(value))
    }

    pub(crate) async fn send_json(&mut self, value: &impl serde::Serialize) -> Result<()> {
        let message = to_string(value).map(Message::Text)?;

        self.0.send(message).await?;
        Ok(())
    }

    /// Delegate to `StreamExt::next`
    #[cfg(feature = "client")]
    pub(crate) async fn next(&mut self) -> Option<std::result::Result<Message, WsError>> {
        self.0.next().await
    }

    /// Delegate to `SinkExt::send`
    #[cfg(feature = "client")]
    pub(crate) async fn send(&mut self, message: Message) -> Result<()> {
        self.0.send(message).await?;
        Ok(())
    }

    /// Delegate to `WebSocketStream::close`
    #[cfg(feature = "client")]
    pub(crate) async fn close(&mut self, msg: Option<CloseFrame<'_>>) -> Result<()> {
        self.0.close(msg).await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Errors if there is a problem with the WS connection.
    pub async fn send_chunk_guild(
        &mut self,
        guild_id: GuildId,
        shard_info: &ShardInfo,
        limit: Option<u16>,
        presences: bool,
        filter: ChunkGuildFilter,
        nonce: Option<&str>,
    ) -> Result<()> {
        debug!("[{:?}] Requesting member chunks", shard_info);

        let (query, user_ids) = match filter {
            ChunkGuildFilter::None => (Some(String::new()), None),
            ChunkGuildFilter::Query(query) => (Some(query), None),
            ChunkGuildFilter::UserIds(user_ids) => (None, Some(user_ids)),
        };

        self.send_json(&WebSocketMessage {
            op: Opcode::RequestGuildMembers,
            d: WebSocketMessageData::ChunkGuild(ChunkGuildMessage {
                guild_id,
                query: query.as_deref(),
                limit: limit.unwrap_or(0),
                presences,
                user_ids,
                nonce: nonce.unwrap_or(""),
            }),
        })
        .await
    }

    /// # Errors
    ///
    /// Errors if there is a problem with the WS connection.
    pub async fn request_soundboard_sounds(
        &mut self,
        guild_ids: &[GuildId],
        shard_info: &ShardInfo,
    ) -> Result<()> {
        debug!("[{:?}] Requesting soundboard sounds", shard_info);

        self.send_json(&WebSocketMessage {
            op: Opcode::ReqeustSoundboardSounds,
            d: WebSocketMessageData::SoundboardSounds {
                guild_ids,
            },
        })
        .await
    }

    /// # Errors
    ///
    /// Errors if there is a problem with the WS connection.
    #[instrument(skip(self))]
    pub async fn send_heartbeat(&mut self, shard_info: &ShardInfo, seq: Option<u64>) -> Result<()> {
        trace!("[{:?}] Sending heartbeat d: {:?}", shard_info, seq);

        self.send_json(&WebSocketMessage {
            op: Opcode::Heartbeat,
            d: WebSocketMessageData::Heartbeat(seq),
        })
        .await
    }

    /// # Errors
    ///
    /// Errors if there is a problem with the WS connection.
    #[instrument(skip(self, token))]
    pub async fn send_identify(
        &mut self,
        shard: &ShardInfo,
        token: &str,
        intents: GatewayIntents,
        presence: &PresenceData,
    ) -> Result<()> {
        let activities: Vec<_> = presence.activity.iter().collect();
        let now = SystemTime::now();

        debug!("[{:?}] Identifying", shard);

        let msg = WebSocketMessage {
            op: Opcode::Identify,
            d: WebSocketMessageData::Identify {
                token,
                shard,
                intents,
                compress: true,
                large_threshold: constants::LARGE_THRESHOLD,
                properties: IdentifyProperties {
                    browser: "serenity",
                    device: "serenity",
                    os: consts::OS,
                },
                presence: PresenceUpdateMessage {
                    afk: false,
                    since: now,
                    status: presence.status.name(),
                    activities: &activities,
                },
            },
        };

        self.send_json(&msg).await
    }

    /// # Errors
    ///
    /// Errors if there is a problem with the WS connection.
    #[instrument(skip(self))]
    pub async fn send_presence_update(
        &mut self,
        shard_info: &ShardInfo,
        presence: &PresenceData,
    ) -> Result<()> {
        let activities: Vec<_> = presence.activity.iter().collect();
        let now = SystemTime::now();

        debug!("[{:?}] Sending presence update", shard_info);

        self.send_json(&WebSocketMessage {
            op: Opcode::PresenceUpdate,
            d: WebSocketMessageData::PresenceUpdate(PresenceUpdateMessage {
                afk: false,
                since: now,
                status: presence.status.name(),
                activities: &activities,
            }),
        })
        .await
    }

    /// # Errors
    ///
    /// Errors if there is a problem with the WS connection.
    #[instrument(skip(self, token))]
    pub async fn send_resume(
        &mut self,
        shard_info: &ShardInfo,
        session_id: &str,
        seq: u64,
        token: &str,
    ) -> Result<()> {
        debug!("[{:?}] Sending resume; seq: {}", shard_info, seq);

        self.send_json(&WebSocketMessage {
            op: Opcode::Resume,
            d: WebSocketMessageData::Resume {
                session_id,
                token,
                seq,
            },
        })
        .await
    }
}

fn proxy_from_env(url: &Url) -> Option<Url> {
    proxy_from_lookup(url, |key| env::var(key).ok())
}

fn proxy_from_lookup<F>(url: &Url, mut lookup: F) -> Option<Url>
where
    F: FnMut(&str) -> Option<String>,
{
    let no_proxy = lookup("NO_PROXY")
        .or_else(|| lookup("no_proxy"))
        .unwrap_or_default();
    if no_proxy_matches_raw(url, &no_proxy) {
        return None;
    }

    let keys: &[&str] = match url.scheme() {
        "wss" | "https" => &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"],
        "ws" | "http" => &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"],
        _ => &["ALL_PROXY", "all_proxy"],
    };

    for key in keys {
        let value = match lookup(key) {
            Some(value) if !value.trim().is_empty() => value,
            _ => continue,
        };

        match Url::parse(value.trim()) {
            Ok(proxy) if proxy.scheme() == "http" => return Some(proxy),
            Ok(proxy) => {
                warn!(env = *key, proxy = %proxy, "unsupported proxy scheme for Discord gateway; only http:// proxies are supported");
            },
            Err(error) => {
                warn!(env = *key, value = value.trim(), %error, "invalid proxy URL for Discord gateway");
            },
        }
    }

    None
}

fn no_proxy_matches_raw(url: &Url, raw: &str) -> bool {
    let host = match url.host_str() {
        Some(host) => host,
        None => return false,
    };
    let port = url.port_or_known_default();

    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| no_proxy_entry_matches(host, port, entry))
}

fn no_proxy_entry_matches(host: &str, port: Option<u16>, entry: &str) -> bool {
    if entry == "*" {
        return true;
    }

    let (entry_host, entry_port) = split_no_proxy_entry(entry);
    if let Some(entry_port) = entry_port {
        if Some(entry_port) != port {
            return false;
        }
    }

    let entry_host = entry_host.trim_matches('[').trim_matches(']').trim_start_matches('.');
    if entry_host.is_empty() {
        return false;
    }

    let host = host.trim_matches('[').trim_matches(']');
    host.eq_ignore_ascii_case(entry_host)
        || host
            .to_ascii_lowercase()
            .ends_with(&format!(".{}", entry_host.to_ascii_lowercase()))
}

fn split_no_proxy_entry(entry: &str) -> (&str, Option<u16>) {
    if let Some(stripped) = entry.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            let host = &stripped[..end];
            let rest = &stripped[end + 1..];
            let port = rest.strip_prefix(':').and_then(|value| value.parse().ok());
            return (host, port);
        }
    }

    match entry.rsplit_once(':') {
        Some((host, value)) if !host.contains(':') => (host, value.parse().ok()),
        _ => (entry, None),
    }
}

async fn connect_via_proxy(url: &Url, proxy: &Url) -> std::result::Result<TcpStream, WsError> {
    let proxy_host = proxy
        .host_str()
        .ok_or_else(|| WsError::Io(io_error("proxy URL is missing a hostname")))?;
    let proxy_port = proxy
        .port_or_known_default()
        .ok_or_else(|| WsError::Io(io_error("proxy URL is missing a port")))?;
    let target_host = url
        .host_str()
        .ok_or_else(|| WsError::Io(io_error("gateway URL is missing a hostname")))?;
    let target_port = url
        .port_or_known_default()
        .ok_or_else(|| WsError::Io(io_error("gateway URL is missing a port")))?;

    let mut stream = TcpStream::connect((proxy_host, proxy_port)).await.map_err(WsError::Io)?;
    let authority = format!("{target_host}:{target_port}");
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");

    if !proxy.username().is_empty() {
        let credentials = format!(
            "{}:{}",
            proxy.username(),
            proxy.password().unwrap_or_default()
        );
        let encoded = base64::prelude::BASE64_STANDARD.encode(credentials);
        request.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }

    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.map_err(WsError::Io)?;
    read_connect_response(&mut stream).await?;
    Ok(stream)
}

async fn read_connect_response(stream: &mut TcpStream) -> std::result::Result<(), WsError> {
    let mut response = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 512];

    loop {
        let read = stream.read(&mut chunk).await.map_err(WsError::Io)?;
        if read == 0 {
            return Err(WsError::Io(io_error(
                "proxy closed the connection before completing CONNECT",
            )));
        }

        response.extend_from_slice(&chunk[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if response.len() > 16 * 1024 {
            return Err(WsError::Io(io_error(
                "proxy CONNECT response headers exceeded 16 KiB",
            )));
        }
    }

    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| idx + 4)
        .ok_or_else(|| WsError::Io(io_error("proxy CONNECT response was incomplete")))?;
    let head = String::from_utf8_lossy(&response[..header_end]);
    let status_line = head.lines().next().unwrap_or_default();

    if status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200") {
        return Ok(());
    }

    Err(WsError::Io(io_error(format!(
        "proxy CONNECT request failed: {status_line}"
    ))))
}

fn build_ws_request(
    url: &Url,
    proxy: &Url,
) -> std::result::Result<tokio_tungstenite::tungstenite::http::Request<()>, WsError> {
    let mut request = url.as_str().into_client_request()?;
    if !proxy.username().is_empty() {
        let credentials = format!(
            "{}:{}",
            proxy.username(),
            proxy.password().unwrap_or_default()
        );
        let encoded = base64::prelude::BASE64_STANDARD.encode(credentials);
        let header_value = format!("Basic {encoded}")
            .parse()
            .map_err(|_| WsError::Io(io_error("invalid proxy authorization header")))?;
        request
            .headers_mut()
            .insert("Proxy-Authorization", header_value);
    }
    Ok(request)
}

fn io_error(message: impl Into<String>) -> IoError {
    IoError::new(IoErrorKind::Other, message.into())
}

#[cfg(test)]
mod tests {
    use super::{no_proxy_matches_raw, proxy_from_lookup, split_no_proxy_entry};
    use url::Url;

    #[test]
    fn no_proxy_supports_exact_hosts_and_suffixes() {
        let url = Url::parse("wss://gateway.discord.gg/?v=10&encoding=json").unwrap();
        assert!(no_proxy_matches_raw(&url, "gateway.discord.gg"));
        assert!(no_proxy_matches_raw(&url, ".discord.gg"));
        assert!(no_proxy_matches_raw(&url, "discord.gg"));
        assert!(!no_proxy_matches_raw(&url, "example.com"));
    }

    #[test]
    fn no_proxy_supports_ports() {
        let url = Url::parse("https://discord.com:443/api/v10/gateway").unwrap();
        assert!(no_proxy_matches_raw(&url, "discord.com:443"));
        assert!(!no_proxy_matches_raw(&url, "discord.com:8443"));
    }

    #[test]
    fn split_no_proxy_entry_handles_ipv6_ports() {
        assert_eq!(split_no_proxy_entry("[::1]:8080"), ("::1", Some(8080)));
        assert_eq!(split_no_proxy_entry("discord.com:443"), ("discord.com", Some(443)));
        assert_eq!(split_no_proxy_entry("discord.com"), ("discord.com", None));
    }

    #[test]
    fn proxy_lookup_prefers_scheme_specific_values() {
        let url = Url::parse("wss://gateway.discord.gg/?v=10&encoding=json").unwrap();
        let proxy = proxy_from_lookup(&url, |key| match key {
            "HTTPS_PROXY" => Some("http://proxy-a:8080".into()),
            "ALL_PROXY" => Some("http://proxy-b:8080".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(proxy.as_str(), "http://proxy-a:8080/");
    }
}
