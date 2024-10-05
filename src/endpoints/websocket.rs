use std::net::SocketAddr;
use async_recursion::async_recursion;
use tokio::sync::mpsc;

use axum::{extract::{ws::{Message, WebSocket}, ConnectInfo, State, WebSocketUpgrade}, response::IntoResponse};
use tracing::{info, warn, error};
use futures::{sink::SinkExt, stream::{SplitStream, StreamExt}};

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
    stream: WebSocket,
    addr: SocketAddr
) {
    let (mut sender, mut reciever) = stream.split();

    if sender.send(Message::Ping(vec![1, 2, 3])).await.is_err() {
        info!("[{}] Unable to ping browser, terminating connection!", addr);
        return;
    }

    let session_id = find_socket_id(&mut reciever).await;
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
        let _ = sender.send(Message::Text("duplicate-connection".to_string())).await;
        return;
    }

    clients.insert(session_id.clone(), tx);
    drop(clients);

    let mut rx_task = tokio::spawn(async move {
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

            if sender.send(Message::Text(message)).await.is_err() {
                error!("[{}] There was an error while trying to send converter response!", addr); 
            } else {
                info!("[{}] Sent response with converted response!", addr);
            }
        }
    });

    let mut socket_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = reciever.next().await {
            let data = extract_message_data(msg);
            match data {
                Either::Right(should_close) => {
                    let should_close = should_close.into();
                    if should_close {
                        break
                    }
                },
                _ => continue
            }
        }
    });

    tokio::select! {
        _ = &mut rx_task => socket_task.abort(),
        _ = &mut socket_task => rx_task.abort()
    };

    let mut clients = state.connected_clients.write().await;
    clients.remove(&session_id);
    drop(clients);

    info!("[{}] Socket with ID {} disconnected!", addr, session_id);
}

#[async_recursion]
async fn find_socket_id(reciever: &mut SplitStream<WebSocket>) -> Option<String> {
    if let Some(Ok(msg)) = reciever.next().await {
        let data = extract_message_data(msg);
        match data {
            Either::Left(message) => {
                let socket_id = message.split(';').last().unwrap();
                return Some(socket_id.into())
            },
            _ => find_socket_id(reciever).await
        }
    } else {
        None
    }
}

fn extract_message_data(msg: Message) -> Either<String, ShouldSocketClose> {
    match msg {
        Message::Text(t) => return Either::Left(t),
        Message::Close(_) => return Either::Right(true.into()),
        _ => return Either::Right(false.into())
    }
}

struct ShouldSocketClose(bool);
impl Into<bool> for ShouldSocketClose {
    
    fn into(self) -> bool {
        return self.0
    }

}

impl From<bool> for ShouldSocketClose {
   
    fn from(value: bool) -> Self {
        return Self(value)
    }

}

enum Either<T, V> {
    Left(T),
    Right(V)
}
