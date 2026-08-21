import { createReadStream, statSync } from "node:fs"
import { createServer } from "node:http"
import { extname, join, normalize } from "node:path"

const root = new URL("../out/", import.meta.url).pathname
const port = Number.parseInt(process.env.PORT ?? "4173", 10)
const mimeTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".woff2", "font/woff2"],
])

createServer((request, response) => {
  const pathname = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname)
  const relative = normalize(pathname).replace(/^([/\\])+/, "").replace(/^app[/\\]?/, "")
  let file = join(root, relative)

  try {
    if (statSync(file).isDirectory()) file = join(file, "index.html")
    const stat = statSync(file)
    response.writeHead(200, {
      "content-length": stat.size,
      "content-type": mimeTypes.get(extname(file)) ?? "application/octet-stream",
    })
    createReadStream(file).pipe(response)
  } catch {
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" })
    response.end("Not found")
  }
}).listen(port, "127.0.0.1", () => {
  process.stdout.write(`Cortex static export listening on http://127.0.0.1:${port}\n`)
})
