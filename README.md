# opencode-iroh

Minimal Node/Bun binding for [iroh](https://github.com/n0-computer/iroh), built with [napi-rs](https://napi.rs). It exposes exactly what OpenCode's peer service needs: one endpoint, outbound and inbound connections, and bi-directional streams.

The endpoint uses iroh's minimal preset with only your own relays (`relayUrls`, none by default; never n0's public relays) and no public address lookup, so the application carries endpoint addresses itself. It is built against a [patched iroh 1.4.1](#patched-iroh).

## Install

Distributed through [GitHub Releases](https://github.com/kommander/opencode-iroh/releases) only; nothing is published to npm (`@opencode-ai/iroh` is just the package name).

```sh
bun add https://github.com/kommander/opencode-iroh/releases/download/v0.4.0/opencode-ai-iroh-0.4.0.tgz
```

The main tarball holds the loader and type declarations. Its `optionalDependencies` point at that release's platform tarballs (`opencode-ai-iroh-<platform>-<version>.tgz`, one `.node` each), and the loader picks the one matching the host. Bun does not filter tarball dependencies by `os`/`cpu`, so it downloads all eight platform tarballs (about 25 MB). The lockfile pins each tarball's integrity, so release assets must never be replaced.

## API

```ts
import { bind } from "@opencode-ai/iroh"

const relayUrls = ["https://relay.example.com"] // own relays admitting clients by bearer token; omit for none
const endpoint = await bind({ secretKey, alpn: "opencode/peer/1", idleTimeoutMs: 10_000, relayUrls }) // secretKey: 32 bytes
await endpoint.setRelayToken(token) // the relays stay unused until a token is set; call again to renew, null removes them
endpoint.id() // hex public key
// In the background: renew the token when a relay denies it ({ url, reason }); null once the endpoint is closed.
;(async () => { while (await endpoint.relayAuthDenied()) await endpoint.setRelayToken(await fetchToken()) })()
await endpoint.online() // resolves once a home relay is connected (never without relays)
endpoint.addr() // { id, relay: string | null, addresses: string[] }
endpoint.boundSockets() // e.g. ["0.0.0.0:51234", "[::]:51235"]

// Outbound: dial by id plus any known relay URL and direct addresses.
const connection = await endpoint.connect({ id, relay, addresses })
const { send, recv } = await connection.openBi()
await send.writeAll(new TextEncoder().encode("hello"))
await send.finish()
const reply = await recv.readToEnd(4096) // Buffer

// Inbound: accept() resolves to null once the endpoint is closed.
for (let incoming; (incoming = await endpoint.accept()); ) {
  const connection = await incoming.accept() // TLS handshake; remoteId() is authenticated
  const stream = await connection.acceptBi()
  const chunk = await stream.recv.read(64 * 1024) // empty Buffer when the stream finished
}

connection.closeReason() // null while open
connection.close(0, "bye")
await endpoint.close()
```

`idleTimeoutMs` (optional) closes a connection once the peer has been silent that long, so a dead peer is noticed within roughly `idleTimeoutMs` to 4/3 × `idleTimeoutMs`; keep-alives every `idleTimeoutMs / 3` keep healthy idle connections open. Keep the `Endpoint` referenced while its connections are in use: if it is garbage collected, its connections close.

`relayUrls` (optional) are the endpoint's relays; iroh's default QUIC address discovery (UDP port 7842) runs against them. Unset means no relays at all: peers are reachable only by direct addresses. The relays join the endpoint's relay map only on `setRelayToken(token)`, which sends `token` as the bearer token when connecting; `setRelayToken(null)` removes them from the map. Every relay connect and reconnect uses the current token, so renewing it with `setRelayToken(newToken)` needs no re-bind: an established relay connection keeps running, and once it drops (or while it is being denied) the next attempt, at most iroh's reconnect backoff (≤ 16 s) later, uses the new token.

`relayAuthDenied()` tells when to renew: it resolves with `{ url, reason }` when the home relay denies the current token, on the first connection attempt or on a reconnect, and with null once the endpoint is closed. It resolves once per token: iroh keeps retrying the denied token with its backoff, but those denials are not reported again until `setRelayToken` is called and a later attempt is denied (one attempt already under way during `setRelayToken` may still carry the old token). A denial that happened while nobody was waiting is reported to the next call; every call pending at that moment resolves with it, so an abandoned call (for example one that lost a `Promise.race`) does not swallow it.

`send.reset(code?)` and `recv.stop(code?)` abort a stream and interrupt a pending `writeAll` / `read`. See [`index.d.ts`](index.d.ts) for the full declarations.

## Build

Requires Rust 1.93 (pinned in `rust-toolchain.toml`) and Bun.

```sh
bun install
bun run build   # release build for the host: iroh.<platform>.node, index.js, index.d.ts
bun test        # needs cargo to run the local relay (examples/relay.rs); no test contacts an internet relay
```

`index.js` (platform loader) and `index.d.ts` are generated by `napi build` and committed; CI fails if they are stale.

## Patched iroh

[`patches/iroh`](patches/iroh) is the published iroh 1.4.1 crate source (`Cargo.toml`, `build.rs`, `src/`, licence and readme; examples, tests and docs left out) with one change, applied through `[patch.crates-io]` in `Cargo.toml`.

Why: iroh 1.4.1 reads a relay's auth token from the relay map once, when it starts the relay connection (`RelayActor::start_active_relay`), bakes it into the connection's `ClientBuilder`, and reuses that builder for every reconnect. An expired token can therefore never be replaced without re-binding the endpoint, even though `Endpoint::insert_relay` updates the map.

The change (`src/socket/transports/relay/actor.rs`): `RelayConnectionOptions` carries the shared `RelayMap` instead of the token, `ActiveRelayActor` keeps it, and `dial_relay` looks up the relay's current `auth_token` on every dial. Nothing else changes; a relay missing from the map is dialed without a token, as before. It is small enough to propose upstream as is. To see it: `diff -ru ~/.cargo/registry/src/*/iroh-1.4.1/src patches/iroh/src`. When upgrading iroh, replace `patches/iroh` with the new release's source and reapply the change, or drop the patch once upstream has an equivalent.

## CI and releases

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) builds the addon for linux x64/arm64 (gnu and musl), macOS x64/arm64 and Windows x64/arm64, uploads each as a `bindings-<target>` artifact, and runs the tests (including a local relay built from `examples/relay.rs`) on linux x64, macOS arm64 and Windows x64.

Pushing a `v<version>` tag that matches `package.json` runs the `release` job: it packs one tarball per platform (`napi create-npm-dirs` + `napi artifacts` + `npm pack`), packs the main package with `optionalDependencies` pointing at them, and creates the GitHub Release with all nine tarballs. To release, bump `version` in `package.json` and `Cargo.toml`, rebuild so the generated `index.js` picks up the new version, commit, then tag and push.
