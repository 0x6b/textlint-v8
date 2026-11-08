import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { after, test } from 'node:test';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temp = mkdtempSync(join(tmpdir(), 'textlint-v8-npm-'));
after(() => rmSync(temp, { recursive: true, force: true }));
const platform = `${process.platform}-${process.arch}`;
const executable = process.platform === 'win32' ? 'textlint-v8.exe' : 'textlint-v8';
const binary = resolve(process.env.TEXTLINT_V8_BINARY || join(root, 'target/debug', executable));
const env = { ...process.env, npm_config_cache: join(temp, 'cache') };

function run(command, args, options = {}) {
  if (process.platform === 'win32' && ['npm', 'npx'].includes(command)) {
    args = [join(dirname(process.execPath), `node_modules/npm/bin/${command}-cli.js`), ...args];
    command = process.execPath;
  }
  const result = spawnSync(command, args, { cwd: temp, env, encoding: 'utf8', ...options });
  assert.ifError(result.error);
  return result;
}

function success(command, args) {
  const result = run(command, args);
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}

test('packed packages work through npx without install scripts or registry access', async t => {
  success(process.execPath, [join(root, 'build/package.mjs'), platform, binary, join(temp, 'packages')]);
  const tarballs = ['textlint-v8', `textlint-v8-${platform}`].map(name => {
    const packed = JSON.parse(success('npm', ['pack', join(temp, 'packages', name), '--json', '--ignore-scripts']));
    return join(temp, packed[0].filename);
  });
  success('npm', ['install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund', ...tarballs]);
  const manifest = JSON.parse(readFileSync(join(temp, 'node_modules/@0x6b/textlint-v8/package.json')));
  const execute = (args, options) => run('npx', ['--offline', '@0x6b/textlint-v8', ...args], options);

  await t.test('version matches Cargo package', () => {
    const result = execute(['--version']);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), `textlint-v8 ${manifest.version}`);
  });
  await t.test('stdin, arguments, diagnostics and lint exit code are preserved', () => {
    const result = execute(['--stdin', '--stdin-filename', 'a document.md', '--format', 'json'], { input: 'Hello 😀\n' });
    assert.equal(result.status, 1, result.stderr);
    const files = JSON.parse(result.stdout);
    assert.ok(files[0].messages.some(message => message.ruleId.includes('no-emoji')));
  });
  await t.test('fix output is preserved', () => {
    const result = execute(['--stdin', '--stdin-filename', 'document.md', '--fix', '--format', 'fixed-result'], { input: '**Hello**\n' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'Hello\n');
  });
  await t.test('invalid CLI options preserve stderr and exit code', () => {
    const result = execute(['--not-a-real-option']);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /--not-a-real-option/);
  });
  const shutdowns = process.platform === 'win32' ? ['EOF'] : ['EOF', 'SIGINT', 'SIGTERM'];
  for (const shutdown of shutdowns) {
    await t.test(`MCP stdio works and shuts down on ${shutdown}`, async () => {
      const launcher = join(temp, 'node_modules/@0x6b/textlint-v8/bin/textlint-v8.cjs');
      const child = spawn(process.execPath, [launcher, '--mcp'], { cwd: temp, env });
      const lines = createInterface({ input: child.stdout });
      const exited = once(child, 'exit');
      const timer = setTimeout(() => {
        child.kill('SIGKILL');
        child.stdin.destroy();
      }, 15000);
      try {
        const response = Promise.race([
          once(lines, 'line'),
          exited.then(() => { throw new Error('MCP exited before responding'); }),
        ]);
        child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'npm-test', version: '1.0.0' } } })}\n`);
        const [output] = await response;
        assert.equal(JSON.parse(output).id, 1);
        assert.ok(JSON.parse(output).result.serverInfo);
        if (shutdown === 'EOF') {
          child.stdin.end(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n`);
          assert.deepEqual(await exited, [0, null]);
        } else {
          child.kill(shutdown);
          assert.deepEqual(await exited, [null, shutdown]);
        }
      } finally {
        lines.close();
        child.stdin.destroy();
        clearTimeout(timer);
        if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      }
    });
  }
  await t.test('missing optional package gives an actionable error', () => {
    rmSync(join(temp, `node_modules/@0x6b/textlint-v8-${platform}`), { recursive: true });
    const result = execute(['--version']);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /--include=optional/);
  });
});
