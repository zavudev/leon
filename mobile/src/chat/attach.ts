// Files attached to a message: what the agent is told of them, and the names
// and bytes of what was picked or pasted. The files themselves are kept on
// the computer before the message goes (`keepAttachment`); the message names
// their paths there, the way the desktop does.

/** What comes between the words of a message and the paths of its files. */
const ATTACHED = '\n\nAttached (open each path):';

/**
 * What is said with files attached: the words, then the files' paths, one a
 * line, for the agent to open. The desktop writes the same
 * (`king::with_attached`), so an agent reads one format wherever it was
 * written. Empty when there is nothing to say.
 */
export function withAttached(typed: string, paths: readonly string[]): string {
  const text = typed.trim();
  if (paths.length === 0) return text;
  const lines = paths.map((path) => `\n- ${path}`).join('');
  return `${text || 'Look at the attached files.'}${ATTACHED}${lines}`;
}

/**
 * A message as [`withAttached`] wrote it, read back: the words and the paths
 * of the files. A message with no files is its words alone.
 */
export function splitAttached(text: string): { text: string; files: string[] } {
  const at = text.lastIndexOf(ATTACHED);
  if (at < 0) return { text, files: [] };
  const files = text
    .slice(at + ATTACHED.length)
    .split('\n')
    .flatMap((line) => (line.startsWith('- ') ? [line.slice(2)] : []));
  if (files.length === 0) return { text, files: [] };
  const words = text.slice(0, at);
  return { text: words === 'Look at the attached files.' ? '' : words, files };
}

/** A kept file's own name: its path's last part, without the stamp it was kept with. */
export function fileLabel(path: string): string {
  const last = path.split(/[\\/]/).pop() ?? path;
  return last.replace(/^\d{10,}-/, '');
}

const EXTENSIONS: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/jpg': 'jpg',
  'image/webp': 'webp',
  'image/gif': 'gif',
  'image/heic': 'heic',
  'image/heif': 'heif',
  'application/pdf': 'pdf',
  'text/plain': 'txt',
};

/**
 * The name a picked file is sent with: its own when it has one with an
 * extension, otherwise `fallback` with the extension its type calls for.
 */
export function nameOf(given: string | null | undefined, mime: string | null | undefined, fallback: string): string {
  const name = given?.split(/[\\/]/).pop()?.trim();
  if (name && /\.[A-Za-z0-9]{1,8}$/.test(name)) return name;
  const extension = mime ? EXTENSIONS[mime.toLowerCase()] : undefined;
  const base = name || fallback;
  return extension ? `${base}.${extension}` : base;
}

/** The bytes of base64 text, with or without a `data:` prefix. */
export function bytesOfBase64(text: string): ArrayBuffer {
  const data = text.replace(/^data:[^,]*,/, '');
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let at = 0; at < binary.length; at++) bytes[at] = binary.charCodeAt(at);
  return bytes.buffer;
}

/** The type a `data:` URL says it holds. */
export function mimeOfDataUrl(text: string): string | undefined {
  return /^data:([^;,]+)/.exec(text)?.[1];
}

/** The most a file may weigh; the computer refuses more. */
export const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;

/** A weight as people read it. */
export function weight(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
