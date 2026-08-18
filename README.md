# textlint-v8

Rust から V8 上の textlint を呼び出す、組み込み用ハーネスです。実行時に Node.js、`node_modules`、ファイルシステム上の JavaScript は必要ありません。

V8はJITを使用するため、実行環境で実行可能メモリの割り当てが許可されている必要があります。`v8` crateのビルド済み静的ライブラリはビルド時に取得され、リリースバイナリへリンクされます。

現在は Markdown と次のルールを静的に組み込んでいます。

- `@0x6b/textlint-rule-no-emoji`
- `@0x6b/textlint-rule-no-emphasis`
- `@0x6b/textlint-rule-no-hr-before-heading`
- `@0x6b/textlint-rule-no-numbered-headings-and-bullets`
- `@0x6b/textlint-rule-no-smart-quotes`
- `@0x6b/textlint-rule-normalize-whitespaces`
- `@textlint-ja/textlint-rule-preset-ai-writing`
- `textlint-rule-preset-ja-technical-writing`

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

## JavaScript bundle の更新

Node.js は組み込み bundle を生成するときだけ使用します。生成済みの `dist/textlint-v8.js` は Rust バイナリへ `include_str!` で埋め込まれます。

ルール設定は `textlint-v8.config.json` に記述します。通常の textlint と同様に、値には `true`、`false`、またはルール固有のオプションオブジェクトを指定できます。設定にないルールは無効です。このファイルもJavaScript bundleへ埋め込まれるため、実行時には必要ありません。

```json
{
  "rules": {
    "@0x6b/no-emoji": true,
    "@0x6b/no-emphasis": false
  }
}
```

```console
npm ci
npm run build
cargo test
```

別の設定を埋め込む場合は、ビルド時にパスを指定できます。

```console
TEXTLINT_V8_CONFIG=config/strict.json npm run build
cargo build --release
```

`js/assert-shim.cjs` は、`@textlint/kernel` が AST 検証に使用する Node の `node:assert` のうち、必要な機能だけを提供します。

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

形態素解析を使う子ルールのため、Kuromojiの圧縮済み辞書をRustバイナリへ埋め込んでいます。辞書はRust側で展開し、V8へ`ArrayBuffer`として渡すため、実行時のファイルアクセスやNode APIは発生しません。辞書のライセンスとNOTICEは`resources/kuromoji`に収録しています。
