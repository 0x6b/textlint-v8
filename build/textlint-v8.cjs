#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const { join, dirname } = require('node:path');
const { optionalDependencies } = require('../package.json');

const name = `@0x6b/textlint-v8-${process.platform}-${process.arch}`;
if (!optionalDependencies[name]) {
  console.error(`textlint-v8: unsupported platform ${process.platform}-${process.arch}. Use cargo install textlint-v8 instead.`);
  process.exit(1);
}

let binary;
try {
  const executable = process.platform === 'win32' ? 'textlint-v8.exe' : 'textlint-v8';
  binary = join(dirname(require.resolve(`${name}/package.json`)), 'bin', executable);
} catch (error) {
  console.error(`textlint-v8: missing ${name}. Reinstall with optional dependencies enabled (npm install --include=optional).`);
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });
const interrupt = () => child.kill('SIGINT');
const terminate = () => child.kill('SIGTERM');
process.on('SIGINT', interrupt);
process.on('SIGTERM', terminate);
child.on('error', error => {
  console.error(`textlint-v8: ${error.message}`);
  process.exitCode = 1;
});
child.on('exit', (code, signal) => {
  process.removeListener('SIGINT', interrupt);
  process.removeListener('SIGTERM', terminate);
  if (signal) process.kill(process.pid, signal);
  else process.exitCode = code;
});
