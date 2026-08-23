use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use uuid::Uuid;

use crate::protocol::{CollectorEvent, CollectorStateMessage, EventReport, ObservationState};
use crate::realtime::response::ServerMessage;

pub const COLLECTOR_PROTOCOL_VERSION: u16 = 1;

const VERSION_HEADER: HeaderName = HeaderName::from_static("x-mnemos-collector-version");
const PROTOCOL_HEADER: HeaderName = HeaderName::from_static("x-mnemos-collector-protocol");
const PLATFORM_HEADER: HeaderName = HeaderName::from_static("x-mnemos-collector-platform");

#[derive(Debug, Clone)]
pub struct RealtimeConfig {
    pub endpoint: String,
    pub response_timeout: Duration,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            endpoint: "wss://api.knalis.rest/ws/v1/collector".to_owned(),
            response_timeout: Duration::from_secs(5),
        }
    }
}

pub struct RealtimeClient {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    response_timeout: Duration,
}

impl RealtimeClient {
    pub async fn connect(config: &RealtimeConfig, access_key: &str) -> Result<Self> {
        let mut request = config
            .endpoint
            .as_str()
            .into_client_request()
            .context("invalid realtime WebSocket endpoint")?;

        let authorization = HeaderValue::from_str(&format!("Collector {access_key}"))
            .context("collector access key cannot be encoded as an HTTP header")?;
        let version = HeaderValue::from_static(env!("CARGO_PKG_VERSION"));
        let protocol = HeaderValue::from_str(&COLLECTOR_PROTOCOL_VERSION.to_string())
            .context("collector protocol version cannot be encoded as an HTTP header")?;
        let platform = HeaderValue::from_str(&format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH,
        ))
        .context("collector platform cannot be encoded as an HTTP header")?;

        request.headers_mut().insert(AUTHORIZATION, authorization);
        request.headers_mut().insert(VERSION_HEADER, version);
        request.headers_mut().insert(PROTOCOL_HEADER, protocol);
        request.headers_mut().insert(PLATFORM_HEADER, platform);

        let (socket, _) = connect_async(request)
            .await
            .context("failed to connect to Mnemos realtime-service")?;

        Ok(Self {
            socket,
            response_timeout: config.response_timeout,
        })
    }

    pub async fn report(
        &mut self,
        event: CollectorEvent,
        observed_at: DateTime<Utc>,
    ) -> Result<Uuid> {
        self.set_state(ObservationState::Observing).await?;

        let report = EventReport::new(event, observed_at);
        let message_id = report.message_id;

        self.send_json(&report).await?;
        self.wait_for_report_queued(message_id).await?;

        Ok(message_id)
    }

    pub async fn pause(&mut self) -> Result<()> {
        self.set_state(ObservationState::Paused).await
    }

    pub async fn close(mut self) -> Result<()> {
        self.socket
            .close(None)
            .await
            .context("failed to close realtime WebSocket")
    }

    async fn set_state(&mut self, expected: ObservationState) -> Result<()> {
        let message = match expected {
            ObservationState::Observing => CollectorStateMessage::observing(),
            ObservationState::Paused => CollectorStateMessage::paused(),
        };

        self.send_json(&message).await?;

        timeout(self.response_timeout, async {
            loop {
                match self.next_server_message().await? {
                    ServerMessage::CollectorStateUpdated { state, .. } if state == expected => {
                        return Ok(());
                    }
                    ServerMessage::Error { code, message } => {
                        bail!("realtime-service rejected collector state: {code:?}: {message}");
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| anyhow!("timed out waiting for collector state acknowledgement"))?
    }

    async fn wait_for_report_queued(&mut self, expected_message_id: Uuid) -> Result<()> {
        timeout(self.response_timeout, async {
            loop {
                match self.next_server_message().await? {
                    ServerMessage::ReportQueued { message_id, .. }
                        if message_id == expected_message_id =>
                    {
                        return Ok(());
                    }
                    ServerMessage::Error { code, message } => {
                        bail!("realtime-service rejected event report: {code:?}: {message}");
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| anyhow!("timed out waiting for report acknowledgement"))?
    }

    async fn send_json(&mut self, payload: &impl Serialize) -> Result<()> {
        let json =
            serde_json::to_string(payload).context("failed to serialize collector message")?;

        self.socket
            .send(Message::Text(json.into()))
            .await
            .context("failed to send collector message")
    }

    async fn next_server_message(&mut self) -> Result<ServerMessage> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| anyhow!("realtime WebSocket closed"))?
                .context("realtime WebSocket transport failed")?;

            match message {
                Message::Text(payload) => {
                    return serde_json::from_str(payload.as_str())
                        .context("realtime-service returned an invalid collector message");
                }
                Message::Ping(payload) => {
                    self.socket
                        .send(Message::Pong(payload))
                        .await
                        .context("failed to answer realtime heartbeat")?;
                }
                Message::Close(frame) => {
                    bail!("realtime-service closed collector connection: {frame:?}");
                }
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }
}
