const codes: Record<string, number> = {
  bold: 1,
  underline: 4,
  red: 31,
  green: 32,
  yellow: 33,
  gray: 90,
};

export function styleText(format: string | string[], text: string): string {
  const formats = Array.isArray(format) ? format : [format];
  return `\x1b[${formats.map((value) => codes[value]).join(";")}m${text}\x1b[0m`;
}

export default { styleText };
