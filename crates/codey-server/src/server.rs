//! WebSocket server implementation
//!
//! Accepts WebSocket connections and spawns sessions for each.

use std::net::SocketAddr;

use anyhow::Result;
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use codey::AgentRuntimeConfig;

use crate::protocol::{ClientMessage, ServerMessage};
use crate::session::Session;

/// Server configuration
#[derive(Clone)]
pub struct ServerConfig {
    /// System prompt for agents
    pub system_prompt: String,
    /// Agent runtime configuration
    pub agent_config: AgentRuntimeConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            system_prompt: "You are a helpful AI coding assistant.".to_string(),
            agent_config: AgentRuntimeConfig::default(),
        }
    }
}

/// WebSocket server
pub struct Server {
    addr: SocketAddr,
    config: ServerConfig,
}

impl Server {
    /// Create a new server
    pub fn new(addr: SocketAddr, config: ServerConfig) -> Self {
        Self { addr, config }
    }

    /// Run the server
    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(&self.addr).await?;
        tracing::info!("WebSocket server listening on {}", self.addr);

        while let Ok((stream, peer_addr)) = listener.accept().await {
            tracing::info!("New connection from {}", peer_addr);
            let config = self.config.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, config).await {
                    tracing::error!("Connection error from {}: {}", peer_addr, e);
                }
                tracing::info!("Connection closed: {}", peer_addr);
            });
        }

        Ok(())
    }
}

/// Handle a single WebSocket connection
async fn handle_connection(stream: TcpStream, config: ServerConfig) -> Result<()> {
    let ws_stream = accept_async(stream).await?;
    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Channels for session <-> WebSocket communication
    let (tx_to_ws, mut rx_to_ws) = mpsc::unbounded_channel::<ServerMessage>();
    let (tx_from_ws, rx_from_ws) = mpsc::unbounded_channel::<ClientMessage>();

    // Spawn WebSocket writer task
    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = rx_to_ws.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::error!("Failed to serialize message: {}", e);
                    continue;
                }
            };
            if let Err(e) = ws_sink.send(Message::Text(json.into())).await {
                tracing::error!("Failed to send WebSocket message: {}", e);
                break;
            }
        }
    });

    // Spawn WebSocket reader task
    let reader_tx = tx_from_ws.clone();
    let reader_handle = tokio::spawn(async move {
        while let Some(msg) = ws_source.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(client_msg) => {
                            if reader_tx.send(client_msg).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse client message: {}", e);
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    tracing::debug!("Client sent close frame");
                    break;
                }
                Ok(Message::Ping(data)) => {
                    tracing::trace!("Received ping");
                    // Pong is automatically sent by tungstenite
                    let _ = data;
                }
                Ok(_) => {
                    // Ignore binary, pong, etc.
                }
                Err(e) => {
                    tracing::error!("WebSocket error: {}", e);
                    break;
                }
            }
        }
    });

    // Create and run session
    let mut session = Session::new(
        config.agent_config,
        &config.system_prompt,
        tx_to_ws,
        rx_from_ws,
    );

    tracing::info!("Session {} started", session.id());
    let result = session.run().await;
    tracing::info!("Session {} ended", session.id());

    // Clean up
    writer_handle.abort();
    reader_handle.abort();

    result
}
