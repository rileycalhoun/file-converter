use std::net::SocketAddr;

use axum::{extract::{ws::{Message, WebSocket}, ConnectInfo, State, WebSocketUpgrade}, response::IntoResponse};
use axum_extra::TypedHeader;
use headers::UserAgent;
use tower_sessions::{session::Id, Session};
use tracing::{debug, info};

use crate::SharedState;

pub async fn socket(
    ws: WebSocketUpgrade, // the actual websocket
    user_agent: Option<TypedHeader<UserAgent>>, // the browser the client is using (Firefox,
                                                // Chrome)
    ConnectInfo(addr): ConnectInfo<SocketAddr>,

    session: Session,
    State(state): State<SharedState>
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string() // Firefox, Chrome
    } else {
        String::from("Unknown browser")
    };

    info!("[{}] {} connected.", addr, user_agent);
    
    if session.id().is_none() {
        let _ = session.save().await;
    }

    let id = session.id().unwrap();
    ws.on_upgrade(move |socket| handle_socket(id, state, socket, addr))
}

pub async fn handle_socket(id: Id, state: SharedState, mut socket: WebSocket, who: SocketAddr) {
    if socket.send(Message::Ping(vec![1, 2, 3])).await.is_ok() {
        debug!("[{}] Successfully pinged!", who);
    } else {
        debug!("[{}] Could not ping!", who);

        // Return since we cannot message the client
        return;
    }

    let mut state = state.lock().await;
    let connected = &mut state.connected_sockets;
   
    if connected.contains_key(&id) {
        connected.remove(&id);
    }

    // Socket should not be dropped until it is removed from the HashMap
    connected.insert(id, socket);
}
