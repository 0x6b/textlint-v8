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
```

Lint formatters are `checkstyle`, `compact`, `github`, `jslint-xml`, `json`, `junit`, `pretty-error`, `stylish` (default), `table`, `tap`, and `unix`. Fix formatters are `compats`, `diff`, `fixed-result`, `json`, and `stylish` (default). With files, `--fix` writes changes back unless `--dry-run` is set.

### CLI compatibility

`textlint-v8` supports textlint 15.8.0's normal file, directory, glob, stdin, ignore, fix, formatter, and output workflows. `--experimental` is accepted as a compatibility no-op.

Rule and plugin configuration is intentionally replaced by the embedded rule set. Cache, debug logging, and dynamically loaded external formatters are not supported.

### Model Context Protocol

`textlint-v8 --mcp` runs a stdio MCP server with `lintFile`, `lintText`,
`getLintFixedFileContent`, and `getLintFixedTextContent` tools. The tool names
and inputs match textlint 15.8.0. Results use a stable, LLM-oriented schema with
per-file diagnostics and summary counts; fix tools include the fixed content and
never modify files on disk.

## Rust API

When using `textlint-v8` as a library, disable the default `cli` feature:

```toml
[dependencies]
textlint-v8 = { version = "0.1.0", default-features = false }
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

## License

The original source code in this repository is licensed under the MIT License.
See [LICENSE](./LICENSE) for details.

Distributed binaries also contain third-party Rust crates, V8, bundled npm
packages, and Kuromoji/IPADIC dictionary data under their respective licenses.
Run `textlint-v8 --licenses` to print the license and attribution notices
embedded in the binary.
