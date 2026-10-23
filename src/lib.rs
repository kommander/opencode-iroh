use std::{fmt::Display, net::SocketAddr, str::FromStr, time::Duration};

use iroh::endpoint::{self, QuicTransportConfig, VarInt, presets};
use iroh::{EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr};
use napi::bindgen_prelude::{Buffer, Uint8Array};
use napi_derive::napi;
use tokio::sync::{Mutex, Notify};

fn error(e: impl Display) -> napi::Error {
  napi::Error::from_reason(e.to_string())
}

fn var_int(value: Option<u32>) -> VarInt {
  VarInt::from_u32(value.unwrap_or(0))
}

#[napi(object)]
pub struct BindOptions {
  /// 32-byte ed25519 secret key; the endpoint id is its public key.
  pub secret_key: Uint8Array,
  /// The single ALPN this endpoint accepts and dials.
  pub alpn: String,
  /// Closes connections after this long without hearing from the peer; keep-alives every third of it keep live idle
  /// connections open. Unset keeps iroh's defaults.
  pub idle_timeout_ms: Option<u32>,
}

/// How to reach an endpoint: its id plus an optional home relay URL and direct `ip:port` addresses.
#[napi(object, use_nullable = true)]
pub struct EndpointAddr {
  pub id: String,
  pub relay: Option<String>,
  pub addresses: Vec<String>,
}

/// Binds an endpoint with the n0 default relays and no public address lookup.
#[napi]
pub async fn bind(options: BindOptions) -> napi::Result<Endpoint> {
  let key: [u8; 32] = options
    .secret_key
    .as_ref()
    .try_into()
    .map_err(|_| error("secretKey must be 32 bytes"))?;
  let alpn = options.alpn.into_bytes();
  let mut builder = iroh::Endpoint::builder(presets::Minimal)
    .secret_key(SecretKey::from_bytes(&key))
    .alpns(vec![alpn.clone()])
    .relay_mode(RelayMode::Default);
  if let Some(ms) = options.idle_timeout_ms {
    let transport = QuicTransportConfig::builder()
      .max_idle_timeout(Some(VarInt::from_u32(ms).into()))
      .keep_alive_interval(Duration::from_millis(u64::from(ms / 3)))
      .build();
    builder = builder.transport_config(transport);
  }
  let inner = builder.bind().await.map_err(error)?;
  Ok(Endpoint { inner, alpn })
}

#[napi]
pub struct Endpoint {
  inner: iroh::Endpoint,
  alpn: Vec<u8>,
}

#[napi]
impl Endpoint {
  /// Hex-encoded public key.
  #[napi]
  pub fn id(&self) -> String {
    self.inner.id().to_string()
  }

  #[napi]
  pub fn addr(&self) -> EndpointAddr {
    let addr = self.inner.addr();
    EndpointAddr {
      id: addr.id.to_string(),
      relay: addr.relay_urls().next().map(|url| url.to_string()),
      addresses: addr.ip_addrs().map(|ip| ip.to_string()).collect(),
    }
  }

  /// Local UDP sockets, possibly wildcard (`0.0.0.0:port`, `[::]:port`).
  #[napi]
  pub fn bound_sockets(&self) -> Vec<String> {
    self
      .inner
      .bound_sockets()
      .iter()
      .map(|addr| addr.to_string())
      .collect()
  }

  /// Resolves once a home relay is connected.
  #[napi]
  pub async fn online(&self) {
    self.inner.online().await
  }

  /// Dials `addr` with the endpoint's ALPN. The TLS handshake proves the remote holds `addr.id`'s key.
  #[napi]
  pub async fn connect(&self, addr: EndpointAddr) -> napi::Result<Connection> {
    let id = EndpointId::from_str(&addr.id).map_err(error)?;
    let mut addrs = Vec::with_capacity(addr.addresses.len() + 1);
    if let Some(relay) = addr.relay {
      addrs.push(TransportAddr::Relay(
        RelayUrl::from_str(&relay).map_err(error)?,
      ));
    }
    for ip in addr.addresses {
      addrs.push(TransportAddr::Ip(SocketAddr::from_str(&ip).map_err(error)?));
    }
    let addr = iroh::EndpointAddr::from_parts(id, addrs);
    let inner = self.inner.connect(addr, &self.alpn).await.map_err(error)?;
    Ok(Connection { inner })
  }

  /// Next incoming connection attempt, or null once the endpoint is closed.
  #[napi]
  pub async fn accept(&self) -> Option<Incoming> {
    let incoming = self.inner.accept().await?;
    Some(Incoming {
      inner: std::sync::Mutex::new(Some(incoming)),
    })
  }

  #[napi]
  pub async fn close(&self) {
    self.inner.close().await
  }
}

#[napi]
pub struct Incoming {
  inner: std::sync::Mutex<Option<endpoint::Incoming>>,
}

#[napi]
impl Incoming {
  /// Completes the handshake. Can be called once.
  #[napi]
  pub async fn accept(&self) -> napi::Result<Connection> {
    let incoming = self.inner.lock().unwrap().take();
    let incoming = incoming.ok_or_else(|| error("already accepted"))?;
    let inner = incoming.await.map_err(error)?;
    Ok(Connection { inner })
  }
}

#[napi]
pub struct Connection {
  inner: endpoint::Connection,
}

#[napi]
impl Connection {
  #[napi]
  pub fn remote_id(&self) -> String {
    self.inner.remote_id().to_string()
  }

  #[napi]
  pub async fn open_bi(&self) -> napi::Result<BiStream> {
    let (send, recv) = self.inner.open_bi().await.map_err(error)?;
    Ok(BiStream::new(send, recv))
  }

  #[napi]
  pub async fn accept_bi(&self) -> napi::Result<BiStream> {
    let (send, recv) = self.inner.accept_bi().await.map_err(error)?;
    Ok(BiStream::new(send, recv))
  }

  #[napi]
  pub fn close(&self, code: Option<u32>, reason: Option<String>) {
    self
      .inner
      .close(var_int(code), reason.unwrap_or_default().as_bytes())
  }

  /// Null while the connection is open.
  #[napi]
  pub fn close_reason(&self) -> Option<String> {
    self.inner.close_reason().map(|reason| reason.to_string())
  }
}

#[napi(object, object_from_js = false)]
pub struct BiStream {
  pub send: SendStream,
  pub recv: RecvStream,
}

impl BiStream {
  fn new(send: endpoint::SendStream, recv: endpoint::RecvStream) -> Self {
    Self {
      send: SendStream {
        inner: Mutex::new(send),
        cancel: Notify::new(),
      },
      recv: RecvStream {
        inner: Mutex::new(recv),
        cancel: Notify::new(),
      },
    }
  }
}

// `cancel` lets reset/stop interrupt a pending write/read, which would otherwise hold the lock indefinitely.

#[napi]
pub struct SendStream {
  inner: Mutex<endpoint::SendStream>,
  cancel: Notify,
}

#[napi]
impl SendStream {
  #[napi]
  pub async fn write_all(&self, data: Uint8Array) -> napi::Result<()> {
    let mut send = self.inner.lock().await;
    tokio::select! {
      result = send.write_all(&data) => result.map_err(error),
      _ = self.cancel.notified() => Err(error("stream reset")),
    }
  }

  /// Gracefully ends the stream once all written data is delivered.
  #[napi]
  pub async fn finish(&self) -> napi::Result<()> {
    self.inner.lock().await.finish().map_err(error)
  }

  /// Abandons the stream; unsent data is discarded.
  #[napi]
  pub async fn reset(&self, code: Option<u32>) -> napi::Result<()> {
    self.cancel.notify_one();
    self.inner.lock().await.reset(var_int(code)).map_err(error)
  }
}

#[napi]
pub struct RecvStream {
  inner: Mutex<endpoint::RecvStream>,
  cancel: Notify,
}

#[napi]
impl RecvStream {
  /// Reads up to `max` bytes; an empty buffer means the stream finished.
  #[napi]
  pub async fn read(&self, max: u32) -> napi::Result<Buffer> {
    let mut recv = self.inner.lock().await;
    let mut buf = vec![0; max as usize];
    let read = tokio::select! {
      result = recv.read(&mut buf) => result.map_err(error)?,
      _ = self.cancel.notified() => return Err(error("stream stopped")),
    };
    buf.truncate(read.unwrap_or(0));
    Ok(buf.into())
  }

  /// Reads until the stream finishes, failing if it exceeds `max` bytes.
  #[napi]
  pub async fn read_to_end(&self, max: u32) -> napi::Result<Buffer> {
    let mut recv = self.inner.lock().await;
    tokio::select! {
      result = recv.read_to_end(max as usize) => result.map(Into::into).map_err(error),
      _ = self.cancel.notified() => Err(error("stream stopped")),
    }
  }

  /// Tells the peer to stop sending.
  #[napi]
  pub async fn stop(&self, code: Option<u32>) -> napi::Result<()> {
    self.cancel.notify_one();
    self.inner.lock().await.stop(var_int(code)).map_err(error)
  }
}
