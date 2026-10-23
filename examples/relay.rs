//! Relay server for the tests: plain HTTP on a random local port, admitting only the bearer token "good". Each line
//! on stdin restarts it on the same port, dropping all connections, admitting only the token on that line.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use iroh_relay::server::{Access, AccessControl, ClientRequest, RelayConfig, Server, ServerConfig};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Debug)]
struct Token(String);

impl AccessControl for Token {
  async fn on_connect(&self, request: &ClientRequest) -> Access {
    match request.auth_token() {
      Some(token) if token == self.0 => Access::Allow,
      _ => Access::Deny { reason: None },
    }
  }
}

async fn spawn(addr: SocketAddr, token: String) -> Server {
  let mut relay = RelayConfig::new(addr);
  relay.access = Arc::new(Token(token));
  let mut config = ServerConfig::default();
  config.relay = Some(relay);
  Server::spawn(config).await.expect("relay server")
}

#[tokio::main]
async fn main() {
  let mut server = spawn(([127, 0, 0, 1], 0).into(), "good".into()).await;
  let addr = server.http_addr().expect("http address");
  println!("http://{addr}");
  let mut lines = BufReader::new(tokio::io::stdin()).lines();
  while let Ok(Some(token)) = lines.next_line().await {
    // Shutdown disconnects only registered clients; a client that just completed its handshake may not be yet.
    tokio::time::sleep(Duration::from_millis(500)).await;
    server.shutdown().await.expect("relay shutdown");
    server = spawn(addr, token).await;
    println!("restarted");
  }
  std::future::pending::<()>().await
}
