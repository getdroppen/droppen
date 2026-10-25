// Just enough WebSocket for the fake hub: the RFC 6455 handshake and text
// frames, both directions. No extensions, no fragmentation, no permessage
// deflate — the harness only needs the app's socket to open and one small
// message each way.
import { createHash } from "node:crypto";

export function acceptKey(key) {
  return createHash("sha1").update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
}

/** Complete the handshake on a raw `upgrade` socket; `null` when the request is not one. */
export function handshake(req, socket) {
  const key = req.headers["sec-websocket-key"];
  if (!key || String(req.headers.upgrade ?? "").toLowerCase() !== "websocket") {
    socket.end("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
    return null;
  }
  socket.write(
    [
      "HTTP/1.1 101 Switching Protocols",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Accept: ${acceptKey(key)}`,
      "",
      "",
    ].join("\r\n"),
  );
  socket.setNoDelay(true);
  return { target: req.url, origin: req.headers.origin ?? null, messages: [] };
}

/** A small unmasked (server-to-client) text frame. */
export function sendText(socket, text) {
  const payload = Buffer.from(text, "utf8");
  let header;
  if (payload.length < 126) {
    header = Buffer.from([0x81, payload.length]);
  } else {
    header = Buffer.alloc(4);
    header[0] = 0x81;
    header[1] = 126;
    header.writeUInt16BE(payload.length, 2);
  }
  try {
    socket.write(Buffer.concat([header, payload]));
  } catch {
    // the peer went away
  }
}

export function closeFrame(socket) {
  try {
    socket.write(Buffer.from([0x88, 0x00]));
  } catch {}
}

/**
 * Decode client frames (masked) and call `onText` for each complete text
 * message. Anything malformed closes the socket.
 */
export function readFrames(socket, onText) {
  let buffer = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
      if (buffer.length < 2) return;
      const fin = (buffer[0] & 0x80) !== 0;
      const opcode = buffer[0] & 0x0f;
      const masked = (buffer[1] & 0x80) !== 0;
      let length = buffer[1] & 0x7f;
      let at = 2;
      if (length === 126) {
        if (buffer.length < 4) return;
        length = buffer.readUInt16BE(2);
        at = 4;
      } else if (length === 127) {
        if (buffer.length < 10) return;
        const big = buffer.readBigUInt64BE(2);
        if (big > BigInt(1024 * 1024)) {
          closeFrame(socket);
          socket.destroy();
          return;
        }
        length = Number(big);
        at = 10;
      }
      const maskLength = masked ? 4 : 0;
      if (buffer.length < at + maskLength + length) return;
      const mask = masked ? buffer.subarray(at, at + 4) : null;
      const payload = Buffer.from(buffer.subarray(at + maskLength, at + maskLength + length));
      if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
      buffer = buffer.subarray(at + maskLength + length);
      if (opcode === 0x8) {
        closeFrame(socket);
        socket.destroy();
        return;
      }
      if (opcode === 0x9) {
        try {
          socket.write(Buffer.from([0x8a, payload.length, ...payload]));
        } catch {}
        continue;
      }
      if (opcode === 0x1 && fin) onText(payload.toString("utf8"));
    }
  });
}
