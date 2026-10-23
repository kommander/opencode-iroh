//! Relay server for the tests: plain HTTP on a random local port, admitting only the bearer token "good".

use std::sync::Arc;

use iroh_relay::server::{Access, AccessControl, ClientRequest, RelayConfig, Server, ServerConfig};

#[derive(Debug)]
struct Token;

impl AccessControl for Token {
  async fn on_connect(&self, request: &ClientRequest) -> Access {
    match request.auth_token().as_deref() {
      Some("good") => Access::Allow,
      _ => Access::Deny { reason: None },
    }
  }
}

#[tokio::main]
async fn main() {
  let mut relay = RelayConfig::new(([127, 0, 0, 1], 0));
  relay.access = Arc::new(Token);
  let mut config = ServerConfig::default();
  config.relay = Some(relay);
  let server = Server::spawn(config).await.expect("relay server");
  println!("http://{}", server.http_addr().expect("http address"));
  std::future::pending::<()>().await
}
