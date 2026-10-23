// Runs examples/relay.rs: a local plain-HTTP relay admitting only the bearer token "good".
export async function startRelay() {
  const relay = Bun.spawn(["cargo", "run", "-q", "--example", "relay"], {
    cwd: `${import.meta.dir}/..`,
    stdin: "pipe",
    stdout: "pipe",
    stderr: "inherit",
  })
  const reader = relay.stdout.getReader()
  let output = ""
  async function line() {
    while (!output.includes("\n")) {
      const { value, done } = await reader.read()
      if (done) throw new Error("relay exited")
      output += new TextDecoder().decode(value)
    }
    const end = output.indexOf("\n")
    const result = output.slice(0, end).trim()
    output = output.slice(end + 1)
    return result
  }
  const url = await line()
  return {
    url,
    /** Restarts the relay on the same port, dropping all connections, admitting only `token`. */
    async restart(token: string) {
      relay.stdin.write(`${token}\n`)
      relay.stdin.flush()
      await line()
    },
    kill: () => relay.kill(),
  }
}
