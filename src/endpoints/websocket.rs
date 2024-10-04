use std::net::SocketAddr;
use tokio::sync::mpsc;

use axum::{extract::{ws::{Message, WebSocket}, ConnectInfo, State, WebSocketUpgrade}, response::IntoResponse};
use tower_sessions::{session::Id, Session};
use tracing::info;

use crate::{converter::jobs::JobId, JobStatus, SharedState, SocketMessage};

pub(super) async fn socket(
    State(state): State<SharedState>,
    ws: WebSocketUpgrade,
    session: Session,
    ConnectInfo(addr): ConnectInfo<SocketAddr>    
) -> impl IntoResponse {
    info!("[{}] Recieved socket connection!", addr);

    if session.id().is_none() {
        let _ = session.save().await;
    }

    let id = session.id().unwrap_or_else(|| panic!("[{}] Expected session_id to be some. got none!", addr));

    ws.on_upgrade(move |socket| {
        handle_socket(state, socket, id, addr)
    })
}

async fn handle_socket(
    state: SharedState,
    mut socket: WebSocket,
    id: Id,
    addr: SocketAddr
) {
    if socket.send(Message::Ping(vec![1, 2, 3])).await.is_err() {
        info!("[{}] Unable to ping browser, terminating connection!", addr);
        return;
    }

    let (tx, mut rx) = mpsc::channel::<SocketMessage>(10);
    let mut clients = state.connected_clients.write().await;
    if clients.contains_key(&id) {
        info!("[{}] Client already connected with Session id {}!", addr, id);
        let _ = socket.send(Message::Text("duplicate-connection".to_string())).await;
        return;
    }

    clients.insert(id, tx);
    drop(clients);

    while let Some(msg) = rx.recv().await {
        let status = msg.job_status;
        match status {
            JobStatus::PENDING => continue,
            JobStatus::FAILED => {
                let message = format!("job-failed;{}", msg.file_name);
                let _ = socket.send(Message::Text(message));
            },
            JobStatus::COMPLETED => {
                let JobId(job_id) = msg.job_id;
                let message = format!("job-completed;{}", job_id);
                let _ = socket.send(Message::Text(message));
            }
        };
    }
}
