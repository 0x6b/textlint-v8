function unavailable(): never {
  throw new Error("filesystem access is unavailable in the embedded textlint runtime");
}

export function readFileSync(path: string): string {
  const contents = globalThis.__textlintV8Files.get(String(path));
  if (contents === undefined) unavailable();
  return contents;
}

export function existsSync(path: string): boolean {
  return globalThis.__textlintV8Files.has(String(path));
}
export const createReadStream = unavailable;
export const createWriteStream = unavailable;
export default { readFileSync, existsSync, createReadStream, createWriteStream };

declare global {
  // eslint-disable-next-line no-var
  var __textlintV8Files: Map<string, string>;
}
