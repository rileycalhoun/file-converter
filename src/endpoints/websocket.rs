use std::net::SocketAddr;
use async_recursion::async_recursion;
use tokio::sync::mpsc;

use axum::{extract::{ws::{Message, WebSocket}, ConnectInfo, State, WebSocketUpgrade}, response::IntoResponse};
use tracing::{info, warn, error};

use crate::{JobStatus, SharedState, SocketMessage};

pub(super) async fn socket(
    State(state): State<SharedState>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>    
) -> impl IntoResponse {
    info!("[{}] Recieved socket connection!", addr);
    ws.on_upgrade(move |socket| {
        handle_socket(state, socket, addr)
    })
}

async fn handle_socket(
    state: SharedState,
    mut socket: WebSocket,
    addr: SocketAddr
) {
    if socket.send(Message::Ping(vec![1, 2, 3])).await.is_err() {
        info!("[{}] Unable to ping browser, terminating connection!", addr);
        return;
    }

    let session_id = handle_socket_message(&mut socket).await;
    if session_id.is_none() {
        warn!("[{}] No session_id!", addr);
        return;
    }

    let session_id = session_id.unwrap();
    info!("[{}] Client connected with id: {}!", addr, session_id);

    let (tx, mut rx) = mpsc::channel::<SocketMessage>(10);
    let mut clients = state.connected_clients.write().await;
    if clients.contains_key(&session_id) {
        info!("[{}] Client already connected with Session id {}!", addr, session_id);
        let _ = socket.send(Message::Text("duplicate-connection".to_string())).await;
        return;
    }

    clients.insert(session_id, tx);
    drop(clients);

    let _ = socket.send(Message::Text("Hello, world".to_string())).await;
    while let Some(msg) = rx.recv().await {
        let status = msg.job_status;
        let message = match status {
            JobStatus::PENDING => continue,
            JobStatus::FAILED => {
                format!("job-failed;{}", msg.job_id.0)
            },
            JobStatus::COMPLETED => {
                format!("job-completed;{}", msg.file_id.unwrap())
            }
        };

        if socket.send(Message::Text(message)).await.is_err() {
            error!("[{}] There was an error while trying to send converter response!", addr); 
        } else {
            info!("[{}] Sent response with converted response!", addr);
        }
    }
}

#[async_recursion]
async fn handle_socket_message(socket: &mut WebSocket) -> Option<String> {
    if let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(msg) => {
                if msg.starts_with("session-id") {
                    let id = msg.split(";").last();
                    if id.is_none() {
                        None
                    } else {
                        Some(id.unwrap().to_string())
                    }
                } else {
                    None
                }
            },
            Message::Pong(_) => handle_socket_message(socket).await,
            _ => {
                None
            }
        }
    } else {
        None
    }
}
