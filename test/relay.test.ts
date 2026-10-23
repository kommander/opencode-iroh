import { afterAll, expect, test } from "bun:test"
import { randomBytes } from "node:crypto"
import { bind, type Endpoint } from "../index.js"

const alpn = "opencode-iroh/test"
// examples/relay.rs: a local plain-HTTP relay admitting only the bearer token "good".
const relay = Bun.spawn(["cargo", "run", "-q", "--example", "relay"], {
  cwd: `${import.meta.dir}/..`,
  stdout: "pipe",
  stderr: "inherit",
})
afterAll(() => relay.kill())

const online = (endpoint: Endpoint, ms: number) =>
  Promise.race([endpoint.online().then(() => true), Bun.sleep(ms).then(() => false)])

test("own relays admit only endpoints with a valid token", async () => {
  const url = new TextDecoder().decode((await relay.stdout.getReader().read()).value).trim()
  const relayUrls = [url]
  const [a, b, denied] = await Promise.all(
    [0, 1, 2].map(() => bind({ secretKey: randomBytes(32), alpn, relayUrls })),
  )
  try {
    expect(await online(a, 2000)).toBe(false)

    await denied.setRelayToken("bad")
    await Promise.all([a.setRelayToken("good"), b.setRelayToken("good")])
    await Promise.all([a.online(), b.online()])
    expect(a.addr().relay).toBe(new URL(url).href)
    expect(await online(denied, 2000)).toBe(false)

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
