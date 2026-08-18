# textlint-v8

An opinionated, embeddable textlint CLI and Rust library built on V8. It produces a standalone binary that requires no Node.js, `node_modules`, or JavaScript files at runtime.

Markdown support and the rules in [`textlint-v8.config.json`](./textlint-v8.config.json) are embedded in the binary.

## Build

```console
cargo build --release
```

The executable is written to `target/release/textlint-v8`. Building does not require Node.js or the pnpm CLI.

## Usage

```console
# Lint a file
target/release/textlint-v8 document.md

# Lint multiple files
target/release/textlint-v8 first.md second.md

# Fix problems automatically
target/release/textlint-v8 --fix document.md

# Lint or fix standard input
cat document.md | target/release/textlint-v8
cat document.md | target/release/textlint-v8 --fix

# Select a formatter
target/release/textlint-v8 --formatter json document.md

# Print licenses for embedded dependencies
target/release/textlint-v8 --licenses
```

Available formatters are `stylish` (default), `compact`, `json`, `checkstyle`, `junit`, and `tap`. With files, `--fix` writes changes back to each file. With standard input, it prints the fixed Markdown to standard output.

## Rust API

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

To embed a different configuration, provide its path at build time:

```console
TEXTLINT_V8_CONFIG=config/strict.json cargo build --release
```

## License

MIT. See [LICENSE](./LICENSE) for details.
