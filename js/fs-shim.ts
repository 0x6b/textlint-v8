function unavailable(): never {
  throw new Error("filesystem access is unavailable in the embedded textlint runtime");
}

export const readFileSync = unavailable;
export const existsSync = unavailable;
export const createReadStream = unavailable;
export const createWriteStream = unavailable;
export default { readFileSync, existsSync, createReadStream, createWriteStream };
