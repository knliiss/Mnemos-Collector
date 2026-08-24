use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::protocol::{
    CollectorStateMessage, CollectorUpdateReadyMessage, EventReport, ObservationState,
};
use crate::realtime::response::ServerMessage;

pub const COLLECTOR_PROTOCOL_VERSION: u16 = 1;

const VERSION_HEADER: HeaderName = HeaderName::from_static("x-mnemos-collector-version");
const PROTOCOL_HEADER: HeaderName = HeaderName::from_static("x-mnemos-collector-protocol");
const PLATFORM_HEADER: HeaderName = HeaderName::from_static("x-mnemos-collector-platform");
const INBOUND_BUFFER: usize = 32;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type SocketWriter = SplitSink<Socket, Message>;
type SocketReader = SplitStream<Socket>;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSlotDecision {
    pub granted: bool,
    pub retry_after: Option<Duration>,
}

#[derive(Debug)]
enum InboundEvent {
    Message(ServerMessage),
    Failed(String),
}

pub struct RealtimeClient {
    writer: Arc<Mutex<SocketWriter>>,
    inbound: mpsc::Receiver<InboundEvent>,
    alive: Arc<AtomicBool>,
    reader_task: JoinHandle<()>,
    response_timeout: Duration,
    acknowledged_state: Option<ObservationState>,
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
        let (writer, reader) = socket.split();
        let writer = Arc::new(Mutex::new(writer));
        let alive = Arc::new(AtomicBool::new(true));
        let (inbound_sender, inbound) = mpsc::channel(INBOUND_BUFFER);
        let reader_task = tokio::spawn(read_loop(
            reader,
            Arc::clone(&writer),
            inbound_sender,
            Arc::clone(&alive),
        ));

        Ok(Self {
            writer,
            inbound,
            alive,
            reader_task,
            response_timeout: config.response_timeout,
            acknowledged_state: None,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub async fn observe(&mut self) -> Result<()> {
        if self.acknowledged_state == Some(ObservationState::Observing) {
            return Ok(());
        }

        self.set_state(ObservationState::Observing).await
    }

    pub async fn report(&mut self, report: &EventReport) -> Result<()> {
        self.set_state(ObservationState::Observing).await?;
        self.send_json(report).await?;
        self.wait_for_report_queued(report.message_id).await
    }

    pub async fn request_update_slot(&mut self, version: &str) -> Result<UpdateSlotDecision> {
        if version.is_empty() {
            bail!("collector update version cannot be empty");
        }

        self.send_json(&CollectorUpdateReadyMessage::new(version))
            .await?;

        timeout(self.response_timeout, async {
            loop {
                match self.next_server_message().await? {
                    ServerMessage::CollectorUpdateSlot {
                        granted,
                        retry_after_seconds,
                    } => {
                        return Ok(UpdateSlotDecision {
                            granted,
                            retry_after: retry_after_seconds.map(Duration::from_secs),
                        });
                    }
                    ServerMessage::Error { code, message } => {
                        bail!("realtime-service rejected collector update: {code:?}: {message}");
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| anyhow!("timed out waiting for collector update slot"))?
    }

    pub async fn pause(&mut self) -> Result<()> {
        if self.acknowledged_state == Some(ObservationState::Paused) {
            return Ok(());
        }

        self.set_state(ObservationState::Paused).await
    }

    pub async fn close(self) -> Result<()> {
        self.alive.store(false, Ordering::Release);

        let result = self
            .writer
            .lock()
            .await
            .close()
            .await
            .context("failed to close realtime WebSocket");

        self.reader_task.abort();

        result
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
                        self.acknowledged_state = Some(state);
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

    async fn wait_for_report_queued(&mut self, expected_message_id: uuid::Uuid) -> Result<()> {
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

    async fn send_json(&self, payload: &impl Serialize) -> Result<()> {
        if !self.is_connected() {
            bail!("realtime WebSocket is not connected");
        }

        let json =
            serde_json::to_string(payload).context("failed to serialize collector message")?;

        self.writer
            .lock()
            .await
            .send(Message::Text(json.into()))
            .await
            .context("failed to send collector message")
    }

    async fn next_server_message(&mut self) -> Result<ServerMessage> {
        match self.inbound.recv().await {
            Some(InboundEvent::Message(message)) => Ok(message),
            Some(InboundEvent::Failed(message)) => bail!("realtime WebSocket failed: {message}"),
            None => bail!("realtime WebSocket reader stopped"),
        }
    }
}

impl Drop for RealtimeClient {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.reader_task.abort();
    }
}

async fn read_loop(
    mut reader: SocketReader,
    writer: Arc<Mutex<SocketWriter>>,
    inbound: mpsc::Sender<InboundEvent>,
    alive: Arc<AtomicBool>,
) {
    while let Some(result) = reader.next().await {
        let message = match result {
            Ok(message) => message,
            Err(error) => {
                send_failure(&inbound, error.to_string()).await;
                break;
            }
        };

        match message {
            Message::Text(payload) => {
                let parsed = serde_json::from_str::<ServerMessage>(payload.as_str());

                match parsed {
                    Ok(message) => {
                        if inbound.send(InboundEvent::Message(message)).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        send_failure(
                            &inbound,
                            format!("realtime-service returned an invalid message: {error}"),
                        )
                        .await;
                        break;
                    }
                }
            }
            Message::Ping(payload) => {
                let result = writer.lock().await.send(Message::Pong(payload)).await;

                if let Err(error) = result {
                    send_failure(&inbound, format!("failed to answer heartbeat: {error}")).await;
                    break;
                }
            }
            Message::Close(frame) => {
                send_failure(
                    &inbound,
                    format!("realtime-service closed collector connection: {frame:?}"),
                )
                .await;
                break;
            }
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    alive.store(false, Ordering::Release);
}

async fn send_failure(inbound: &mpsc::Sender<InboundEvent>, message: String) {
    let _ = inbound.send(InboundEvent::Failed(message)).await;
}
