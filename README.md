# textlint-v8

Rust から V8 上の textlint を呼び出す、組み込み用ハーネスです。実行時に Node.js、`node_modules`、ファイルシステム上の JavaScript は必要ありません。

V8はJITを使用するため、実行環境で実行可能メモリの割り当てが許可されている必要があります。`v8` crateのビルド済み静的ライブラリはビルド時に取得され、リリースバイナリへリンクされます。

現在は Markdown と次の公開rule ID / preset IDを静的に組み込んでいます。

- `@0x6b/no-emoji`
- `@0x6b/no-emphasis`
- `@0x6b/no-hr-before-heading`
- `@0x6b/no-numbered-headings-and-bullets`
- `@0x6b/no-smart-quotes`
- `@0x6b/normalize-whitespaces`
- `@textlint-ja/preset-ai-writing`
- `preset-ja-technical-writing`

## CLI

ファイルを指定するか、標準入力へ Markdown を渡します。診断結果は JSON です。複数ファイルを指定した場合は同じ`Textlint`インスタンスを再利用し、結果の配列を出力します。

```console
cargo run -- document.md
cargo run -- first.md second.md
cat document.md | cargo run
```

リリースバイナリの実行に Node.js は必要ありません。

```console
cargo build --release
env -u PATH ./target/release/textlint-v8 document.md
```

## Rust API

```rust
use textlint_v8::Textlint;

let textlint = Textlint::new()?;
let result = textlint.lint("本文 😀", "document.md")?;
for message in result.messages {
    println!("{}:{}: {}", message.line, message.column, message.message);
}
# Ok::<(), anyhow::Error>(())
```

`Textlint` は bundle を一度だけ評価します。同じインスタンスを再利用すると、文書ごとにV8を初期化する必要がありません。

`textlint-rule-no-doubled-conjunctive-particle-ga`は、文ごとにKuromojiキャッシュを迂回する実装になっています。bundle生成時に同じ`kuromojin.tokenize()`キャッシュを使うよう限定的に書き換え、依存パッケージの実装が変わってパッチできなくなった場合はビルドを失敗させます。

## JavaScript bundle の生成

`build.rs` がpnpm 12のRust実装をライブラリとして呼び出してnpm依存を取得し、RolldownのRust APIでCargoの `OUT_DIR/textlint-v8.js` を生成します。外部のpnpm CLIやNode.jsプロセスは起動しません。依存パッケージのlifecycle scriptも無効です。生成済みbundleはRustバイナリへ `include_str!` で埋め込まれます。

pnpm 12は現時点でRust crateをcrates.ioへ公開していないため、PoCでは `pnpm/pnpm` のcommitをGit依存として固定しています。公開crateになっているRolldownも、生成結果の再現性のためバージョンを固定しています。

`textlint-v8.config.json` の `rules` が、bundleに含める公開rule ID / preset IDの正本です。`build.rs` は各IDから標準命名規則でnpm package名を導出し、`package.json` の `dependencies` に固定semverで宣言されていることを検証します。対応するdependencyがない、versionがrange、またはIDを導出できない場合はビルドが失敗します。kernel、Markdown plugin、kuromojiなど、設定にIDがないdependencyをルールとして扱うことはありません。

| 種別 | 公開ID | 導出するpackage | preset子ruleのprefix |
| --- | --- | --- | --- |
| unscoped rule | `foo` | `textlint-rule-foo` | - |
| scoped rule | `@scope/foo` | `@scope/textlint-rule-foo` | - |
| unscoped preset | `preset-foo` | `textlint-rule-preset-foo` | `foo` |
| scoped preset | `@scope/preset-foo` | `@scope/textlint-rule-preset-foo` | `@scope/foo` |

通常のtextlintと同様に、設定値には `true`、`false`、またはルール固有のオプションオブジェクトを指定できます。このファイルはJavaScript bundleへ埋め込まれるため、実行時には必要ありません。検証後、`build.rs` は静的importを持つregistry moduleを `OUT_DIR` へ生成し、Rolldownが `js/index.ts` と一緒にbundleします。registryとbundleは生成物なのでcommitしません。

```json
{
  "rules": {
    "@0x6b/no-emoji": true,
    "@0x6b/no-emphasis": false
  }
}
```

```console
cargo test
```

別の設定を埋め込む場合は、ビルド時にパスを指定できます。

```console
TEXTLINT_V8_CONFIG=config/strict.json cargo build --release
```

`js/assert-shim.cjs` は、`@textlint/kernel` が AST 検証に使用する Node の `node:assert` のうち、必要な機能だけを提供します。

## ルールを追加する

1. 単体ruleまたはpresetの公開IDを `textlint-v8.config.json` の `rules` に追加します。presetは名前を `preset-` で始め、値にはpreset全体の設定を指定します。
2. 上表から導出されるnpm packageを、固定versionで `package.json` の `dependencies` に追加します。
3. lockfileを安全に更新するため、`TEXTLINT_V8_UPDATE_LOCKFILE=1 cargo build` を一度実行します。これは外部pnpm CLIではなく、build scriptが使用するpnpm Rust APIで `pnpm-lock.yaml` を更新します。その後、環境変数なしの通常ビルドがfrozen lockfileで成功することを確認します。
4. packageがNode APIを要求する場合に限り、既存の `js/*-shim.*` とRolldown alias、または限定的なRolldown plugin変換を追加します。形態素解析を使うruleでは、既存のKuromoji辞書loaderとRust bridgeで足りるか確認します。
5. 下記を実行し、診断のrule IDと生成bundleを確認します。

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
rg 'globalThis\.textlintV8' target/debug/build/textlint-v8-*/out/textlint-v8.js
git status --short # OUT_DIRのregistry/bundleが表示されないこと
```

Node.jsなしのclean buildも、Node.jsをインストールしていない環境で `rm -rf node_modules && cargo clean && cargo build --locked` を実行して確認できます。依存取得、lockfile検証、bundle生成はすべてRust内で完結します。

## preset

presetはビルド時に子ルールへ静的展開します。通常のtextlint設定と同じキーでpreset全体を有効化できます。オプションオブジェクトを指定すると、子ルールの無効化や設定の置き換えができます。

```json
{
  "rules": {
    "@textlint-ja/preset-ai-writing": {
      "no-ai-hype-expressions": false
    },
    "preset-ja-technical-writing": {
      "sentence-length": { "max": 120 }
    }
  }
}
```

`true`または指定のない子ルールにはpreset既定値を使います。オプションオブジェクトはpreset既定値とマージせず、通常のtextlintと同様に置き換えます。

形態素解析を使う子ルールのため、pnpmで取得した `kuromoji@0.1.2` の圧縮済み辞書を `OUT_DIR/kuromoji` へコピーし、Rustバイナリへ埋め込んでいます。辞書はRust側で展開し、V8へ`ArrayBuffer`として渡すため、実行時のファイルアクセスやNode APIは発生しません。辞書のライセンスとNOTICEは`resources/kuromoji`に収録しています。
