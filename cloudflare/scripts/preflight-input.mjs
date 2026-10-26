import { constants } from "node:fs";
import { open } from "node:fs/promises";
import { parseTree } from "jsonc-parser";

const MAX_BYTES = 1024 * 1024;

/** Strict JSON: reject duplicate keys, comments, excessive depth and node count. */
export function strictJson(bytes) {
  if (bytes.length > MAX_BYTES) throw new Error("invalid_input");
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes), errors = [];
  const tree = parseTree(text, errors, { allowTrailingComma: false, disallowComments: true });
  if (!tree || errors.length) throw new Error("invalid_input");
  let count = 0;
  function visit(node, depth) {
    if (++count > 20000 || depth > 32) throw new Error("invalid_input");
    if (node.type === "object") {
      const names = new Set();
      for (const property of node.children || []) {
        const key = property.children[0].value;
        if (names.has(key)) throw new Error("invalid_input");
        names.add(key);
      }
    }
    for (const child of node.children || []) visit(child, depth + 1);
  }
  visit(tree, 0); return JSON.parse(text);
}

/** Read only an explicitly supplied private regular file, with a fixed cap. */
export async function readPrivateJson(path) {
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = await file.stat();
    if (!stat.isFile() || stat.size > MAX_BYTES || (stat.mode & 0o077) !== 0) throw new Error("invalid_input");
    const buffer = Buffer.alloc(MAX_BYTES + 1); let size = 0;
    while (size <= MAX_BYTES) {
      const { bytesRead } = await file.read(buffer, size, buffer.length - size, null);
      if (bytesRead === 0) return strictJson(buffer.subarray(0, size));
      size += bytesRead;
    }
    throw new Error("invalid_input");
  } finally { await file.close(); }
}
