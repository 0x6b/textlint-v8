import { chmodSync, copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const platforms = ['linux-x64', 'linux-arm64', 'darwin-arm64', 'win32-x64', 'win32-arm64'];
const [platform, binary, output] = process.argv.slice(2);
if (!platforms.includes(platform) || !binary || !output || process.argv.length !== 5) {
  console.error(`Usage: node build/package.mjs <${platforms.join('|')}> <binary> <output-directory>`);
  process.exit(1);
}

const cargo = readFileSync(join(root, 'Cargo.toml'), 'utf8');
const version = cargo.split('[package]')[1].split('\n[')[0].match(/^version = "([^"]+)"\r?$/m)[1];
const shared = {
  version,
  license: 'MIT',
  repository: { type: 'git', url: 'git+https://github.com/0x6b/textlint-v8.git' },
  publishConfig: { access: 'public' },
};
const [os, cpu] = platform.split('-');
const packages = [
  ['textlint-v8', {
    ...shared,
    name: '@0x6b/textlint-v8',
    description: 'Standalone textlint runner powered by V8',
    engines: { node: '>=18' },
    bin: { 'textlint-v8': 'bin/textlint-v8.cjs' },
    files: ['bin'],
    optionalDependencies: Object.fromEntries(platforms.map(p => [`@0x6b/textlint-v8-${p}`, version])),
  }, join(root, 'build/textlint-v8.cjs'), 'textlint-v8.cjs'],
  [`textlint-v8-${platform}`, {
    ...shared,
    name: `@0x6b/textlint-v8-${platform}`,
    description: `Native textlint-v8 binary for ${platform}`,
    os: [os],
    cpu: [cpu],
    ...(os === 'linux' ? { libc: ['glibc'] } : {}),
    files: ['bin'],
  }, resolve(binary), os === 'win32' ? 'textlint-v8.exe' : 'textlint-v8'],
];

for (const [directory, manifest, source, executable] of packages) {
  const destination = resolve(output, directory);
  mkdirSync(join(destination, 'bin'), { recursive: true });
  writeFileSync(join(destination, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  copyFileSync(source, join(destination, 'bin', executable));
  chmodSync(join(destination, 'bin', executable), 0o755);
  for (const file of ['LICENSE', 'README.md']) copyFileSync(join(root, file), join(destination, file));
  console.log(destination);
}
