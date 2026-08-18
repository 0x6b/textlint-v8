function normalizeParts(parts: string[]): string {
  const output: string[] = [];
  for (const part of parts.join("/").split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      output.pop();
    } else {
      output.push(part);
    }
  }
  return output.join("/");
}

export function join(...parts: string[]): string {
  return normalizeParts(parts);
}

export function resolve(...parts: string[]): string {
  return `/${normalizeParts(parts)}`;
}

export function normalize(value: string): string {
  const prefix = value.startsWith("/") ? "/" : "";
  return `${prefix}${normalizeParts([value])}` || ".";
}

export function dirname(value: string): string {
  const parts = value.split("/");
  parts.pop();
  return parts.join("/") || ".";
}

export default { dirname, join, normalize, resolve, sep: "/" };
