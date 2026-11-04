# Ruby LSP RS for VS Code

`ruby-lsp-rs` を VS Code から使うための拡張です。

## インストール

```sh
cargo build --release      # リポジトリのルートで
cd editors/vscode
npm install
npm run package            # target/release/ruby-lsp-rs を同梱した vsix を作る
code --install-extension ruby-lsp-rs-*.vsix
```

同梱したサーバーはビルドしたマシンと同じ OS・CPU でだけ動きます。別のサーバーを使うときは `rubyLspRs.server.path` を指定してください。

steep-vscode や Shopify Ruby LSP と同時に有効にすると hover や診断が重複するので、ワークスペースで無効にしてください。

## 設定

| 設定 | 既定値 | 内容 |
| --- | --- | --- |
| `rubyLspRs.rubyCommand` | 自動検出 | Ruby の起動コマンド（例: `["mise", "exec", "--", "ruby"]`）。mise / rbenv / asdf は自動で検出します |
| `rubyLspRs.server.path` | 同梱のサーバー | サーバーのパス |
| `rubyLspRs.typeChecker.enable` | `true` | 型推論・型検査 |
| `rubyLspRs.typeChecker.diagnostics.enable` | `true` | 型エラーの診断 |
| `rubyLspRs.definition.target` | `"implementation"` | 定義へ移動の飛び先。`"signature"` で RBS、`"both"` で両方 |
| `rubyLspRs.inlayHints.methodReturnTypes.enable` | `true` | `def` の戻り値の型 |
| `rubyLspRs.inlayHints.methodParameterTypes.enable` | `true` | `def` の引数の型 |
| `rubyLspRs.inlayHints.attributeTypes.enable` | `true` | `attr_*` の型 |
| `rubyLspRs.inlayHints.variableTypes.enable` | `true` | 変数の型 |
| `rubyLspRs.inlayHints.blockParameterTypes.enable` | `true` | ブロック引数の型 |
| `rubyLspRs.inlayHints.hideObviousTypes` | `true` | `Foo.new` やリテラルのように型が明らかなものは表示しない |
| `rubyLspRs.inlayHints.hideUntyped` | `true` | `untyped` は表示しない |
| `rubyLspRs.inlayHints.maxLength` | `30` | hint の最大長 |

`rubyLspRs.enabledFeatures`、`rubyLspRs.formatter`、`rubyLspRs.linters` は Shopify Ruby LSP の `rubyLsp.*` と同じです。

inlay hint は rust-analyzer と同じく、キーを押している間だけ表示を切り替えることもできます。

```jsonc
"[ruby]": {
  "editor.inlayHints.enabled": "offUnlessPressed"
}
```

## 定義へのジャンプ

| 操作 | 飛び先 |
| --- | --- |
| 定義へ移動（`F12` / `Cmd+クリック`） | Ruby の実装。推論したレシーバーの型から探します。実装が無ければ RBS |
| 宣言へ移動 | RBS のシグネチャ |
| 型定義へ移動 | 変数や式の型のクラス |
| hover 内のリンク | `Signature` は RBS、`Implementation` は Ruby の実装 |

RBS ファイルの中で定義へ移動すると、メソッド名・`attr_*`・`alias`・クラス名・定数名からは Ruby の実装へ、型名からは Ruby の実装と RBS の宣言の両方へ飛びます。

## コマンド

| コマンド | 内容 |
| --- | --- |
| `Ruby LSP RS: Restart Server` | サーバーを再起動する |
| `Ruby LSP RS: Show Output` | サーバーのログを表示する |
| `Ruby LSP RS: Toggle Inlay Hints` | inlay hint 全体を切り替える |
| `Ruby LSP RS: Toggle Type Hint Kinds` | 表示する型 hint の種類を選ぶ |

## 開発

`editors/vscode` を VS Code で開き、「Run Extension (debug server)」で起動すると `target/debug/ruby-lsp-rs` を使う Extension Development Host が立ち上がります。
