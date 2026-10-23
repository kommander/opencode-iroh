import { afterAll, expect, test } from "bun:test"
import { randomBytes } from "node:crypto"
import { bind, type Endpoint, type EndpointAddr } from "../index.js"

const alpn = "opencode-iroh/test"
const a = await bind({ secretKey: randomBytes(32), alpn })
const b = await bind({ secretKey: randomBytes(32), alpn })
// A fresh endpoint has no cached paths to b, so the relay-only test really dials via the relay.
const c = await bind({ secretKey: randomBytes(32), alpn })

afterAll(() => Promise.all([a.close(), b.close(), c.close()]))

async function roundtrip(server: Endpoint, client: Endpoint, addr: EndpointAddr) {
  const accepted = server.accept().then((incoming) => incoming!.accept())
  const outbound = await client.connect(addr)
  const inbound = await accepted
  expect(outbound.remoteId()).toBe(server.id())
  expect(inbound.remoteId()).toBe(client.id())

  const stream = await outbound.openBi()
  await stream.send.writeAll(new TextEncoder().encode("ping"))
  await stream.send.finish()
  const remote = await inbound.acceptBi()
  expect(new TextDecoder().decode(await remote.recv.readToEnd(1024))).toBe("ping")
  await remote.send.writeAll(Buffer.from("pong"))
  await remote.send.finish()
  expect((await stream.recv.read(1024)).toString()).toBe("pong")
  expect((await stream.recv.read(1024)).length).toBe(0)

  expect(outbound.closeReason()).toBeNull()
  outbound.close(0, "done")
  expect(outbound.closeReason()).not.toBeNull()
}

test("connects by id and direct addresses", async () => {
  const port = b.boundSockets()[0]!.split(":").at(-1)
  await roundtrip(b, a, { id: b.id(), relay: null, addresses: [`127.0.0.1:${port}`] })
})

test("connects by id and relay only", async () => {
  await b.online()
  const relay = b.addr().relay
  expect(relay).toStartWith("https://")
  await roundtrip(b, c, { id: b.id(), relay, addresses: [] })
}, 30_000)

test("idle timeout keeps idle connections open and detects a dead peer", async () => {
  // The peer runs in a child process so SIGKILL takes it down without a graceful QUIC close.
  const peer = Bun.spawn(
    [
      process.execPath,
      "-e",
      `const { bind } = require(${JSON.stringify(require.resolve("../index.js"))})
      const b = await bind({ secretKey: crypto.getRandomValues(new Uint8Array(32)), alpn: "${alpn}", idleTimeoutMs: 2000 })
      console.log(JSON.stringify({ id: b.id(), port: b.boundSockets()[0].split(":").at(-1) }))
      // Globals keep the endpoint and connection from being garbage collected, which would close them.
      globalThis.peer = { b, connection: await (await b.accept()).accept() }
      setInterval(() => {}, 1000)`,
    ],
    { stdout: "pipe" },
  )
  const { id, port } = JSON.parse(new TextDecoder().decode((await peer.stdout.getReader().read()).value))
  const client = await bind({ secretKey: randomBytes(32), alpn, idleTimeoutMs: 2000 })
  const connection = await client.connect({ id, relay: null, addresses: [`127.0.0.1:${port}`] })
  const pending = connection.acceptBi().catch((error: Error) => error)

  await Bun.sleep(3000)
  expect(connection.closeReason()).toBeNull()

  peer.kill("SIGKILL")
  const killed = Date.now()
  while (connection.closeReason() === null && Date.now() - killed < 5000) await Bun.sleep(50)
  console.log(`closed ${Date.now() - killed} ms after kill: ${connection.closeReason()}`)
  expect(Date.now() - killed).toBeLessThan(3000)
  expect(await pending).toBeInstanceOf(Error)
  await client.close()
}, 15_000)
