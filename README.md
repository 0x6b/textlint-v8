# textlint-v8

Opinionated textlint rules compiled into a standalone binary, with no Node.js or JavaScript package manager required to build or run it.

It runs textlint for Markdown on V8 and provides both a CLI and Rust API. Rules from [`textlint-v8.config.json`](./textlint-v8.config.json) are embedded in the binary.

## Install

```console
cargo install textlint-v8
```

## Build

```console
cargo build --release
```

The executable is written to `target/release/textlint-v8`.

## Usage

```console
# Lint a file
textlint-v8 document.md

# Lint files, directories, or quoted globs
textlint-v8 first.md docs/ "articles/**/*.md"

# Fix problems automatically
textlint-v8 --fix document.md

# Lint or fix standard input
cat document.md | textlint-v8 --stdin --stdin-filename document.md
cat document.md | textlint-v8 --stdin --stdin-filename document.md --fix --format fixed-result

# Select a formatter
textlint-v8 --format json document.md

# Override .textlintignore and disable ANSI colors
textlint-v8 --ignore-path config/textlint.ignore --no-color docs/

# Print licenses for embedded dependencies
textlint-v8 --licenses

# Run the Model Context Protocol server over stdio
textlint-v8 --mcp

# Run the Model Context Protocol server over Streamable HTTP
textlint-v8 --mcp-http 127.0.0.1:3000
```

Lint formatters are `checkstyle`, `compact`, `github`, `jslint-xml`, `json`, `junit`, `pretty-error`, `stylish` (default), `table`, `tap`, and `unix`. Fix formatters are `compats`, `diff`, `fixed-result`, `json`, and `stylish` (default). With files, `--fix` writes changes back unless `--dry-run` is set.

### CLI compatibility

`textlint-v8` supports textlint 15.8.0's normal file, directory, glob, stdin, ignore, fix, formatter, and output workflows. `--experimental` is accepted as a compatibility no-op.

Rule and plugin configuration is intentionally replaced by the embedded rule set. Cache, debug logging, and dynamically loaded external formatters are not supported.

### Model Context Protocol

`textlint-v8 --mcp` runs a stdio MCP server with `lintFile`, `lintText`, `getLintFixedFileContent`, and `getLintFixedTextContent` tools. `--mcp-http ADDRESS` serves the same tools using Streamable HTTP at `/mcp`; `/healthz` is an unauthenticated health check. The tool names and inputs match textlint 15.8.0. Results use a stable, LLM-oriented schema with per-file diagnostics and summary counts; fix tools include the fixed content and never modify files on disk.

The HTTP transport validates the `Host` header to prevent DNS rebinding. Loopback listeners allow the standard loopback hosts by default. A non-loopback listener requires one or more repeatable `--mcp-http-allowed-host HOST[:PORT]` arguments. Requests with an `Origin` header are rejected; non-browser MCP clients such as an internal gateway normally omit it. Put authentication and public TLS at the gateway rather than exposing this backend directly.


## Rust API

When using `textlint-v8` as a library, disable the default `cli` feature:

```toml
[dependencies]
textlint-v8 = { version = "0.2.0", default-features = false }
```

```rust
use textlint_v8::Textlint;

let mut textlint = Textlint::new()?;
let result = textlint.lint("Some text 😀", "document.md")?;

for message in result.messages {
    println!("{}:{}: {}", message.line, message.column, message.message);
}

# Ok::<(), Box<dyn std::error::Error>>(())
```

Reuse the same `Textlint` instance to process multiple documents efficiently. Use `Textlint::fix` to fix problems automatically.

## Adding Rules

To add or change the embedded rules, fork this repository and follow these steps:

1. Add the public rule or preset ID to `rules` in [`textlint-v8.config.json`](./textlint-v8.config.json).
2. Add the corresponding npm package at a fixed version to `dependencies` in [`package.json`](./package.json).
3. Update the lockfile, then verify a normal locked build.

   ```console
   TEXTLINT_V8_UPDATE_LOCKFILE=1 cargo build
   cargo build --locked
   ```

4. If the package uses Node APIs, add only the required functionality to `js/shim/` and the Rolldown aliases. For rules that use morphological analysis, check whether the existing Kuromoji bridge is sufficient.
5. Check formatting, linting, tests, and the generated bundle.

   ```console
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test
   rg 'globalThis\.textlintV8' target/debug/build/textlint-v8-*/out/textlint-v8.js
   ```

Rule IDs map to package names as follows:

| Type | Public ID | npm package |
| --- | --- | --- |
| Rule | `foo` | `textlint-rule-foo` |
| Scoped rule | `@scope/foo` | `@scope/textlint-rule-foo` |
| Preset | `preset-foo` | `textlint-rule-preset-foo` |
| Scoped preset | `@scope/preset-foo` | `@scope/textlint-rule-preset-foo` |

As with standard textlint configuration, each value can be `true`, `false`, or an options object. Preset IDs must start with `preset-`.

If an enabled preset includes a rule that is also registered separately, set that child rule to `false` in the preset. When the same implementation is registered twice, diagnostics use only the rule ID registered first.

## Acknowledgements

This project stands on [textlint](https://github.com/textlint/textlint) and its plugin ecosystem. Thanks to [azu](https://github.com/azu), the textlint maintainers, and the authors and contributors of the [AI writing](https://github.com/textlint-ja/textlint-rule-preset-ai-writing), [Japanese AI words](https://github.com/p1ass/textlint-rule-preset-ai-words-ja), [Japanese technical writing](https://github.com/textlint-ja/textlint-rule-preset-ja-technical-writing), and [invalid control character](https://github.com/textlint-rule/textlint-rule-no-invalid-control-character) rules.

It is also made possible by [Rust](https://www.rust-lang.org/), [V8](https://v8.dev/) and [rusty_v8](https://github.com/denoland/rusty_v8), [Deno](https://github.com/denoland/deno), and [Rolldown](https://github.com/rolldown/rolldown). Thank you to everyone who builds and maintains these projects.

## License

The original source code in this repository is licensed under the MIT License. See [LICENSE](./LICENSE) for details.

Distributed binaries also contain third-party Rust crates, V8, bundled npm packages, and Kuromoji/IPADIC dictionary data under their respective licenses. Run `textlint-v8 --licenses` to print the license and attribution notices embedded in the binary.

## Informal Benchmark

These are directional, not scientific, results from a single Linux x86_64 environment. They compare the release binary with Node.js 24.21.0 and textlint 15.8.0 using the same rule configuration and package versions. Runtime measurements used `hyperfine` with a warm-up run; peak memory is the maximum resident set size reported by GNU `time`.

| Workload | `textlint-v8` | Node.js textlint | Speedup | Peak RSS (binary / Node.js) |
| --- | ---: | ---: | ---: | ---: |
| One Markdown file | 593 ms | 1,329 ms | 2.24x | 275 MiB / 444 MiB |
| 100 files, 880 KB, no diagnostics | 8.48 s | 12.02 s | 1.42x | 488 MiB / 804 MiB |
| 100 files, 898 KB, about 30,000 diagnostics | 12.17 s | 16.73 s | 1.37x | 602 MiB / 910 MiB |

The stripped `textlint-v8` executable was 71 MiB. The equivalent Node.js project's `node_modules` used 127 MiB, excluding the Node.js runtime. Including the 121 MiB Node.js executable raises the minimum runtime footprint to about 248 MiB. Build directories and caches are excluded from both figures.

Actual results depend on the documents, enabled rules, filesystem cache, hardware, and invocation pattern. In particular, reusing a `Textlint` instance through the Rust API avoids paying CLI startup costs for every document.
