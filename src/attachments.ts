/** Files selected in the webview, ready for Loom's Scratch upload API. */
export interface FileAttachment {
  name: string;
  size: number;
  contentBase64: string;
}

const MAX_FILES = 20;
const MAX_FILE_BYTES = 25 * 1024 * 1024;
const MAX_TOTAL_BYTES = 50 * 1024 * 1024;
export const MAX_LAUNCH_TOTAL_BYTES = 45 * 1024 * 1024;

function validName(name: string): boolean {
  return !!name && name === name.trim() && name !== "." && name !== ".."
    && new TextEncoder().encode(name).length <= 240
    && !/[\\/\x00-\x1f\x7f]/.test(name) && name.toLowerCase() !== ".gitignore";
}

function readBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error(`Could not read ${file.name}`));
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== "string" || !result.includes(",")) reject(new Error(`Could not encode ${file.name}`));
      else resolve(result.slice(result.indexOf(",") + 1));
    };
    reader.readAsDataURL(file);
  });
}

export async function addAttachments(existing: FileAttachment[], selected: FileList | File[], maxTotalBytes = MAX_TOTAL_BYTES): Promise<FileAttachment[]> {
  const files = Array.from(selected);
  if (existing.length + files.length > MAX_FILES) throw new Error(`Attach at most ${MAX_FILES} files`);
  const names = new Set(existing.map((file) => file.name));
  let total = existing.reduce((sum, file) => sum + file.size, 0);
  for (const file of files) {
    if (!validName(file.name)) throw new Error(`Invalid attachment name: ${file.name}`);
    if (names.has(file.name)) throw new Error(`Already attached: ${file.name}`);
    if (file.size > MAX_FILE_BYTES) throw new Error(`${file.name} exceeds Loom's 25 MiB file limit`);
    total += file.size;
    if (total > maxTotalBytes) throw new Error(`Attachments exceed the ${maxTotalBytes / 1024 / 1024} MiB total limit`);
    names.add(file.name);
  }
  const additions = await Promise.all(files.map(async (file) => ({
    name: file.name, size: file.size, contentBase64: await readBase64(file),
  })));
  return [...existing, ...additions];
}
