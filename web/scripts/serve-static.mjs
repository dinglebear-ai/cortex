import { createReadStream, statSync } from "node:fs"
import { createServer } from "node:http"
import { dirname, extname, isAbsolute, relative, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptPath = fileURLToPath(import.meta.url)
const root = resolve(dirname(scriptPath), "../out")
const port = Number.parseInt(process.env.PORT ?? "4173", 10)
const mimeTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".woff2", "font/woff2"],
])

const isMissing = (error) =>
  error instanceof Error && "code" in error && ["ENOENT", "ENOTDIR"].includes(error.code)

export function resolveStaticPath(requestUrl, staticRoot = root) {
  let pathname
  try {
    pathname = decodeURIComponent(new URL(requestUrl ?? "/", "http://localhost").pathname)
  } catch (error) {
    throw new TypeError("Malformed request URL", { cause: error })
  }
  if (pathname.includes("\0")) throw new TypeError("Malformed request URL")

  const withoutMount = pathname.replace(/^\/app(?:\/|$)/, "/")
  const candidate = resolve(staticRoot, `.${withoutMount}`)
  const fromRoot = relative(staticRoot, candidate)
  if (fromRoot === ".." || fromRoot.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) || isAbsolute(fromRoot)) {
    throw new TypeError("Request path escapes static root")
  }
  return candidate
}

function sendError(response, status, message) {
  if (response.headersSent) {
    response.destroy()
    return
  }
  response.writeHead(status, { "content-type": "text/plain; charset=utf-8" })
  response.end(message)
}

export function createStaticServer(staticRoot = root) {
  return createServer((request, response) => {
    let file
    try {
      file = resolveStaticPath(request.url, staticRoot)
      if (statSync(file).isDirectory()) file = resolve(file, "index.html")
      const stat = statSync(file)
      const stream = createReadStream(file)
      stream.once("open", () => {
        response.writeHead(200, {
          "content-length": stat.size,
          "content-type": mimeTypes.get(extname(file)) ?? "application/octet-stream",
        })
        stream.pipe(response)
      })
      stream.once("error", (error) => {
        if (!isMissing(error)) console.error("Failed to stream static file", { file, error })
        sendError(response, isMissing(error) ? 404 : 500, isMissing(error) ? "Not found" : "Internal server error")
      })
    } catch (error) {
      if (error instanceof TypeError) {
        console.warn("Rejected malformed static-file request", { url: request.url, error })
        sendError(response, 400, "Bad request")
      } else if (isMissing(error)) {
        sendError(response, 404, "Not found")
      } else {
        console.error("Failed to resolve static file", { file, error })
        sendError(response, 500, "Internal server error")
      }
    }
  })
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  const server = createStaticServer()
  server.on("error", (error) => {
    console.error("Cortex static server failed", error)
    process.exitCode = 1
  })
  server.listen(port, "127.0.0.1", () => {
    process.stdout.write(`Cortex static export listening on http://127.0.0.1:${port}\n`)
  })
}
