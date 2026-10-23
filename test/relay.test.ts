import { afterAll, expect, test } from "bun:test"
import { randomBytes } from "node:crypto"
import { bind, type Endpoint } from "../index.js"
import { startRelay } from "./relay.js"

const alpn = "opencode-iroh/test"
const relay = await startRelay()
afterAll(() => relay.kill())

const online = (endpoint: Endpoint, ms: number) =>
  Promise.race([endpoint.online().then(() => true), Bun.sleep(ms).then(() => false)])
const authDenied = (endpoint: Endpoint, ms: number) =>
  Promise.race([endpoint.relayAuthDenied().then(() => true), Bun.sleep(ms).then(() => false)])

test("own relays admit only endpoints with a valid token", async () => {
  const url = relay.url
  const relayUrls = [url]
  const [a, b, denied] = await Promise.all(
    [0, 1, 2].map(() => bind({ secretKey: randomBytes(32), alpn, relayUrls })),
  )
  try {
    expect(await online(a, 2000)).toBe(false)

    await denied.setRelayToken("bad")
    const rejected = await denied.relayAuthDenied()
    console.log(`first connection attempt denied: ${JSON.stringify(rejected)}`)
    expect(rejected?.url).toBe(new URL(url).href)
    await Promise.all([a.setRelayToken("good"), b.setRelayToken("good")])
    await Promise.all([a.online(), b.online()])
    expect(a.addr().relay).toBe(new URL(url).href)
    expect(await online(denied, 2000)).toBe(false)
    // Reported once per token: the retries with "bad" are not reported again until a new token is set.
    expect(await authDenied(denied, 2000)).toBe(false)
    await denied.setRelayToken("worse")
    expect((await denied.relayAuthDenied())?.url).toBe(new URL(url).href)

    const accepted = b.accept().then((incoming) => incoming!.accept())
    const outbound = await a.connect({ id: b.id(), relay: b.addr().relay, addresses: [] })
    const inbound = await accepted
    const stream = await outbound.openBi()
    await stream.send.writeAll(Buffer.from("ping"))
    await stream.send.finish()
    const remote = await inbound.acceptBi()
    expect((await remote.recv.readToEnd(1024)).toString()).toBe("ping")
    await remote.send.writeAll(Buffer.from("pong"))
    await remote.send.finish()
    expect((await stream.recv.readToEnd(1024)).toString()).toBe("pong")
  } finally {
    await Promise.all([a.close(), b.close(), denied.close()])
  }
}, 60_000)

test("a renewed token reconnects the dropped relay connection without rebinding", async () => {
  const fresh = await startRelay()
  const relayUrls = [fresh.url]
  const [endpoint, peer] = await Promise.all([0, 1].map(() => bind({ secretKey: randomBytes(32), alpn, relayUrls })))
  try {
    await endpoint.setRelayToken("good")
    await endpoint.online()

    // The restart drops the relay connection, and the relay now denies "good" on reconnect.
    await fresh.restart("renewed")
    const rejected = await endpoint.relayAuthDenied()
    console.log(`reconnect denied: ${JSON.stringify(rejected)}`)
    expect(rejected?.url).toBe(new URL(fresh.url).href)
    expect(await online(endpoint, 100)).toBe(false)

    await Promise.all([endpoint.setRelayToken("renewed"), peer.setRelayToken("renewed")])
    const renewed = Date.now()
    await endpoint.online()
    console.log(`online again ${Date.now() - renewed} ms after setRelayToken("renewed")`)

    const accepted = endpoint.accept().then((incoming) => incoming!.accept())
    const outbound = await peer.connect({ id: endpoint.id(), relay: endpoint.addr().relay, addresses: [] })
    expect((await accepted).remoteId()).toBe(peer.id())
    outbound.close(0, "done")

    const closed = endpoint.relayAuthDenied()
    await endpoint.close()
    expect(await closed).toBeNull()
  } finally {
    await Promise.all([endpoint.close(), peer.close()])
    fresh.kill()
  }
}, 60_000)
