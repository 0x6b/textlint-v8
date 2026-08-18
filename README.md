# textlint-v8

Rust から V8 上の textlint を呼び出す、組み込み用ハーネスです。実行時に Node.js、`node_modules`、ファイルシステム上の JavaScript は必要ありません。

V8はJITを使用するため、実行環境で実行可能メモリの割り当てが許可されている必要があります。`v8` crateのビルド済み静的ライブラリはビルド時に取得され、リリースバイナリへリンクされます。

現在は Markdown と [`textlint-v8.config.json`](./textlint-v8.config.json) の公開rule ID / preset IDを静的に組み込んでいます。

## CLI

ファイルを指定するか、標準入力へ Markdown を渡します。既定の出力形式は旧`textlint-standalone`と同じ`stylish`です。`--formatter`では`stylish`、`compact`、`json`、`checkstyle`、`junit`、`tap`を指定できます。複数ファイルを指定した場合は同じ`Textlint`インスタンスを再利用します。

`--fix`をファイルとともに指定すると、修正結果を各ファイルへ書き戻してから診断を表示します。標準入力へ指定した場合は、formatterを使わず修正後のMarkdownだけを標準出力へ書きます。

```console
cargo run -- document.md
cargo run -- --formatter json document.md
cargo run -- --fix document.md
cargo run -- first.md second.md
cat document.md | cargo run
cat document.md | cargo run -- --fix
cargo run -- --licenses
```

### 実行例

既定の`stylish`では、診断位置、メッセージ、rule ID、件数を表示します。

```console
$ printf '本文😀です。\n' | cargo run --quiet

stdin.md
  1:3  error  Found emoji character (\ud83d\ude00)  @0x6b/no-emoji

✖ 1 problem (1 error, 0 warnings, 0 infos)
```

同じ診断をJSONで出力できます。

```console
$ printf '本文😀です。\n' | cargo run --quiet -- --formatter json
[{"filePath":"stdin.md","messages":[{"ruleId":"@0x6b/no-emoji","message":"Found emoji character (\\ud83d\\ude00)","index":2,"line":1,"column":3,"severity":2,"range":[2,3]}]}]
```

標準入力を`--fix`すると、修正後の本文だけを出力します。この例では全角空白が半角空白になります。

```console
$ printf '特殊　空白\n' | cargo run --quiet -- --fix
特殊 空白
```

リリースバイナリの実行に Node.js は必要ありません。

```console
cargo build --release
env -u PATH ./target/release/textlint-v8 document.md
```

## Rust API

```rust
use textlint_v8::{Textlint, third_party_notices};

let mut textlint = Textlint::new()?;
let result = textlint.lint("本文 😀", "document.md")?;
for message in result.messages {
    println!("{}:{}: {}", message.line, message.column, message.message);
}
let fixed = textlint.fix("特殊　空白", "document.md")?;
assert_eq!(fixed.output, "特殊 空白");
println!("{}", third_party_notices());
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Textlint` は bundle を一度だけ評価します。同じインスタンスを再利用すると、文書ごとにV8を初期化する必要がありません。`fix`は修正後の本文、適用した診断、残った診断を返します。`format_lint_results`と`format_fix_results`ではCLIと同じformatterを利用できます。V8 isolateのスレッド制約を型で表すため、`Textlint` は `Send` / `Sync` ではありません。各操作は `&mut self` を要求します。公開APIは `textlint_v8::Error` を返します。このエラーからrequest serialization、V8操作、JavaScript例外、pending Promise、不正なresponseを判別できます。JavaScript例外ではmessageと取得可能なstackを保持します。

`third_party_notices()` は、このbuildの最終成果物に含まれる依存物のライセンスとNOTICEを返します。対象はRust/V8依存、Rolldownが実際にbundleへ出力したnpm package、およびKuromoji辞書です。CLIでは `--licenses`（`--third-party-licenses` も可）で同じ内容を表示します。npm部分は固定lockfileからbuild時に生成し、生成済みNOTICE自体はcommitしません。buildにのみ使うpnpm/RolldownのRust crateは配布バイナリの表示対象に含めません。

`textlint-rule-no-doubled-conjunctive-particle-ga`は、文ごとにKuromojiキャッシュを迂回する実装になっています。bundle生成時に同じ`kuromojin.tokenize()`キャッシュを使うよう限定的に書き換え、依存パッケージの実装が変わってパッチできなくなった場合はビルドを失敗させます。

## JavaScript bundle の生成

`build.rs` がpnpm 12のRust実装をライブラリとして呼び出してnpm依存を取得します。その後、RolldownのRust APIでCargoの `OUT_DIR/textlint-v8.js` を生成します。npm workspace、`node_modules`、pnpm store、生成registry、bundle、辞書コピー、第三者NOTICEはすべて `OUT_DIR` 内で完結します。consumerと依存crateのソースディレクトリへは書き込みません。外部のpnpm CLIやNode.jsプロセスは起動しません。依存パッケージのlifecycle scriptも無効です。生成済みbundleとNOTICEはRustバイナリへ `include_str!` で埋め込まれます。

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

`js/shim/assert.cjs` は、`@textlint/kernel` が AST 検証に使用する Node の `node:assert` のうち、必要な機能だけを提供します。

## ルールを追加する

1. 単体ruleまたはpresetの公開IDを `textlint-v8.config.json` の `rules` に追加する。presetは名前を `preset-` で始め、値にはpreset全体の設定を指定する。
2. 上表から導出されるnpm packageを、固定versionで `package.json` の `dependencies` に追加する。
3. lockfileを安全に更新するため、`TEXTLINT_V8_UPDATE_LOCKFILE=1 cargo build` を一度実行する。これは外部pnpm CLIではなく、build scriptが使用するpnpm Rust APIで `pnpm-lock.yaml` を更新する。その後、環境変数なしの通常ビルドがfrozen lockfileで成功することを確認する。
4. packageがNode APIを要求する場合に限り、既存の `js/shim/` とRolldown alias、または限定的なRolldown plugin変換を追加する。形態素解析を使うruleでは、既存のKuromoji辞書loaderとRust bridgeで足りるか確認する。
5. 下記を実行し、診断のrule IDと生成bundleを確認する。

有効なpresetが同じruleを子ruleとして含む場合は、preset側の子ruleを `false` にします。textlint kernelは、同じrule実装と設定の組み合わせを重複排除します。そのため、両方を有効にすると先に登録されたrule IDだけが診断に使われます。たとえば [`@textlint-rule/no-invalid-control-character`](https://github.com/textlint-rule/textlint-rule-no-invalid-control-character) は `preset-ja-technical-writing` にも含まれます。このリポジトリの設定ではpreset側を無効にしています。

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
rg 'globalThis\.textlintV8' target/debug/build/textlint-v8-*/out/textlint-v8.js
cargo run -- --licenses | rg '@textlint/kernel|mecab-ipadic'
git status --short # OUT_DIRのregistry/bundleが表示されないこと
```

Node.jsなしのclean buildも、Node.jsをインストールしていない環境で `cargo clean && cargo build --locked` を実行して確認できます。依存取得、lockfile検証、bundle生成はすべてRust内で完結し、ソースツリーに `node_modules` やpnpm storeを作りません。

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
