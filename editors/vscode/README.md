# Ruby LSP RS for VS Code

`ruby-lsp-rs`（Shopify Ruby LSP の Rust 移植 + Steep 相当の型推論）を VS Code から使うための拡張です。
型の inlay hint・hover・型診断を提供し、inlay hint は種類ごとに表示・非表示を切り替えられます。

## ビルドとインストール

```sh
# リポジトリのルートでサーバーをビルド
cargo build --release

# 拡張をビルドして vsix を作る
cd editors/vscode
npm install
npm run package

# VS Code にインストール
code --install-extension ruby-lsp-rs-*.vsix
```

`npm run package` は `target/release/ruby-lsp-rs` があればそれを `server/` にコピーして vsix に同梱します。
同梱したサーバーはビルドしたマシンと同じ OS・CPU でのみ動きます。
`target/release/ruby-lsp-rs` が無い場合はサーバーなしの vsix になるので、次節の方法でサーバーを指定してください。

## サーバーの指定方法

次の順に探し、最初に見つかったものを使います。

1. 設定 `rubyLspRs.server.path`（相対パスは最初のワークスペースフォルダ基準、`~` はホームディレクトリに展開。設定した場合は他の候補を探さず、実行できなければエラーにします）
2. 環境変数 `RUBY_LSP_RS_SERVER_PATH`
3. 拡張に同梱した `server/ruby-lsp-rs`
4. `PATH` 上の `ruby-lsp-rs`

```jsonc
// settings.json
{
  "rubyLspRs.server.path": "~/workspace/ruby-lsp-rs/target/release/ruby-lsp-rs",
  "rubyLspRs.server.extraEnv": { "RUST_LOG": "debug" }
}
```

`rubyLspRs.server.path` と `rubyLspRs.server.extraEnv` の変更はサーバーの再起動後に反映されます（変更時に再起動を促す通知が出ます）。

## rubyCommand（Ruby の起動コマンド）

サーバーは Bundler 環境の検出のために Ruby を起動します。
`PATH` 上の `ruby` と Gemfile が要求する Ruby のバージョンが食い違うと Bundler の setup に失敗するため、`rubyLspRs.rubyCommand` で起動コマンドを指定できます。

未設定（`[]`）の場合はワークスペースから自動検出します。

| ワークスペースにあるファイル | 条件 | 使われるコマンド |
| --- | --- | --- |
| `.tool-versions` / `mise.toml` / `.mise.toml` | `mise` がインストール済み | `["mise", "exec", "--", "ruby"]` |
| `.ruby-version` | `rbenv` がインストール済み | `["rbenv", "exec", "ruby"]` |
| `.tool-versions` | `asdf` がインストール済み（mise が無い場合） | `["asdf", "exec", "ruby"]` |

Dock や Finder から起動した VS Code はログインシェルの `PATH` を引き継がないため、`PATH` に無い場合は `~/.local/bin`、`/opt/homebrew/bin`、`~/.rbenv/bin` などの一般的なインストール先も探し、見つかった絶対パスを使います。
検出結果は「Ruby LSP RS」出力に表示されます。

自動検出がうまくいかない場合は明示的に指定してください。

```jsonc
// mise
"rubyLspRs.rubyCommand": ["mise", "exec", "--", "ruby"]

// rbenv
"rubyLspRs.rubyCommand": ["rbenv", "exec", "ruby"]

// asdf
"rubyLspRs.rubyCommand": ["asdf", "exec", "ruby"]

// PATH に無い場合は絶対パスで
"rubyLspRs.rubyCommand": ["/Users/me/.local/bin/mise", "exec", "--", "ruby"]
```

`rubyLspRs.rubyCommand` の変更はサーバーの再起動後に反映されます。

## inlay hint の切り替え

### 全体の表示・非表示

- コマンド `Ruby LSP RS: Toggle Inlay Hints`、またはステータスバーの「Ruby LSP RS」をクリックすると、`[ruby]` 言語スコープの `editor.inlayHints.enabled` を `on` と `off` で切り替えます。
- rust-analyzer と同様に、キーを押している間だけ表示を切り替える設定もできます（macOS は `Ctrl+Option`、Windows / Linux は `Ctrl+Alt`）。

```jsonc
"[ruby]": {
  // 普段は表示し、キーを押している間だけ隠す
  "editor.inlayHints.enabled": "onUnlessPressed"
  // 普段は隠し、キーを押している間だけ表示する
  // "editor.inlayHints.enabled": "offUnlessPressed"
}
```

### 種類ごとの表示・非表示

コマンド `Ruby LSP RS: Toggle Type Hint Kinds` で、次の 4 種類をまとめて選び直せます。設定から個別に変更することもできます。

| 設定 | 対象 |
| --- | --- |
| `rubyLspRs.inlayHints.methodReturnTypes.enable` | `def` の戻り値の型 |
| `rubyLspRs.inlayHints.attributeTypes.enable` | `attr_reader` / `attr_writer` / `attr_accessor` の型 |
| `rubyLspRs.inlayHints.variableTypes.enable` | 変数の型 |
| `rubyLspRs.inlayHints.blockParameterTypes.enable` | ブロックパラメータの型 |

その他の inlay hint 設定:

| 設定 | 既定値 | 内容 |
| --- | --- | --- |
| `rubyLspRs.inlayHints.hideObviousTypes` | `true` | リテラルや `Foo.new` のように右辺から型が明らかなものは表示しない |
| `rubyLspRs.inlayHints.hideUntyped` | `true` | 型が `untyped` のものは表示しない |
| `rubyLspRs.inlayHints.maxLength` | `30` | hint の最大長。`null` で無制限 |
| `rubyLspRs.inlayHints.implicitRescue.enable` | `false` | 引数なしの `rescue` に `StandardError` を表示（再起動後に反映） |
| `rubyLspRs.inlayHints.implicitHashValue.enable` | `false` | 省略されたハッシュの値を表示（再起動後に反映） |

型に関する設定（`rubyLspRs.typeChecker.*` と上の 4 種類・`hideObviousTypes`・`hideUntyped`・`maxLength`）は再起動なしで反映されます。

## 型検査

| 設定 | 既定値 | 内容 |
| --- | --- | --- |
| `rubyLspRs.typeChecker.enable` | `true` | RBS に基づく型推論・型検査を有効にする |
| `rubyLspRs.typeChecker.diagnostics.enable` | `true` | 型エラーを診断として表示する |
| `rubyLspRs.definition.target` | `"implementation"` | 「定義へ移動」「型定義へ移動」の飛び先（次節） |

## 定義・宣言・型定義へのジャンプ

使える移動先は次のとおりです。

| 操作 | 飛び先 |
| --- | --- |
| 定義へ移動（`F12` / `Cmd+クリック`） | メソッド・定数では Ruby の実装（`def` や `class`）。型推論で分かったレシーバの型から引くので、同名メソッドが別のクラスにあっても型に合う方に飛ぶ。変数では変数の定義（下記） |
| 宣言へ移動（コマンドパレットの `Go to Declaration`） | RBS のシグネチャ |
| 型定義へ移動（コマンドパレットの `Go to Type Definition`） | 変数や式の型のクラス・モジュールの定義（下記） |
| hover 内のリンク | `Signature` は RBS のシグネチャ、`Implementation` は Ruby の実装（例: `Signature: sig/user.rbs:6`） |

定義へ移動の細かい挙動:

- 型が分からない位置では、Shopify Ruby LSP と同じく名前で探した候補を返します
- Ruby の実装が無いメソッド（C で書かれた core のメソッドや、RBS だけで宣言したメソッド）は RBS のシグネチャに飛びます
- ローカル変数・ブロック引数・メソッド引数は、その変数の定義（引数ならその位置、それ以外はそのスコープでの最初の代入）に飛びます。ブロックの外の変数をブロックの中で使っている場合は外側の代入に飛びます
- インスタンス変数は、Shopify Ruby LSP と同じ代入箇所（同じクラスとその祖先のメソッド内の代入をすべて）に加えて、RBS の `@name: T` の宣言があればそれも返します。`rubyLspRs.definition.target` が `"signature"` のときは RBS の宣言だけを返します（無ければ代入箇所）
- グローバル変数・`require` などは型推論と関係なく従来どおりです

`rubyLspRs.definition.target` で定義へ移動と型定義へ移動の飛び先を切り替えられます（再起動なしで反映されます）。

| 値 | 飛び先 |
| --- | --- |
| `"implementation"`（既定） | Ruby の実装。無ければ RBS のシグネチャ |
| `"signature"` | RBS のシグネチャ。無ければ Ruby の実装 |
| `"both"` | Ruby の実装と RBS のシグネチャの両方（候補の一覧が表示されます） |

### 変数の型からクラスへ

変数や式に hover すると、型（`Foo?`、`Foo | Bar`、`Array[Foo]` のような union・Optional・型引数の中も含む）に出てくるクラス・モジュールごとに次のリンクが付きます。型にクラスが 2 つ以上あるときは、リンクの前にクラス名が付きます。

- `Signature`: RBS の主たる宣言（1 件）
- `Implementation`: Ruby のクラス定義。クラス名に対応するファイル（`API::OrderItem` なら `api/order_item.rb`）を先頭にし、5 件を超える分は件数だけ表示します

型定義へ移動は、同じクラス・モジュールについて `rubyLspRs.definition.target` に従って飛び先を選びます。Ruby のクラス定義はクラス名に対応するファイルのものがあればそれだけ、無ければすべてを返します。Ruby の定義が無いクラス（`Integer` など）、interface、型エイリアスは RBS の宣言に飛びます。core のクラスは C で書かれているので、gem の中で開き直した定義（`csv/core_ext/array.rb` など）は Ruby の定義として扱いません（ワークスペース内で開き直した定義は扱います）。

RBS の主たる宣言は、クラスを宣言しているすべての RBS ファイルから次の順に比べて選びます。rbs_rails はモデルを関連先のモデルの RBS（`shop/order.rbs` など）でも開き直すため、宣言が何件もあることがあるからです。

1. クラス名に対応するパスのファイル（`api/order_item.rbs` で終わるパス）
2. ファイル名だけがクラス名に対応するファイル（`order_item.rbs`）
3. `rbs_rails` という名前のディレクトリの外にあるファイル（手で書いた `sig/app/models/api/order_item.rbs` を、生成された `sig/rbs_rails/app/models/api/order_item.rbs` より優先する）
4. superclass を書いている宣言（`class Foo < Bar`）
5. それでも決まらなければ、RBS を読み込んだ順で先のもの

宣言へ移動・型定義へ移動には既定のキーが割り当てられていないので、よく使う場合は `editor.action.revealDeclaration` / `editor.action.goToTypeDefinition` にキーボードショートカットを割り当ててください。

## Ruby LSP 互換の設定

`rubyLspRs.enabledFeatures`、`rubyLspRs.formatter`、`rubyLspRs.linters` は Shopify Ruby LSP 拡張の `rubyLsp.enabledFeatures`、`rubyLsp.formatter`、`rubyLsp.linters` と同じ意味・同じ既定値です。変更は再起動後に反映されます。

## steep-vscode / Shopify Ruby LSP との併用

Steep（`soutaro.steep-vscode`）や Ruby LSP（`Shopify.ruby-lsp`）が同時に有効だと、hover・診断・inlay hint が重複して表示されます。
起動時に一度だけ通知が出ます（「Don't Show Again」で以後表示しません）。

この拡張を使うワークスペースでは、拡張機能ビューで該当拡張の「Disable (Workspace)」を選んで無効にすることをおすすめします。
両方を使い続ける場合は、片方の機能を切ってください。例えば Steep の型診断を使い続けるなら:

```jsonc
"rubyLspRs.typeChecker.diagnostics.enable": false
```

## コマンド

| コマンド | 内容 |
| --- | --- |
| `Ruby LSP RS: Restart Server` | サーバーを再起動する |
| `Ruby LSP RS: Show Output` | サーバーのログを表示する |
| `Ruby LSP RS: Toggle Inlay Hints` | Ruby の inlay hint 全体を on / off する |
| `Ruby LSP RS: Toggle Type Hint Kinds` | 型 hint の種類を選ぶ |

通信内容を確認したい場合は `rubyLspRs.trace.server` を `messages` か `verbose` にすると「Ruby LSP RS Trace」出力に表示されます。

## 開発

```sh
cargo build                 # リポジトリのルートで debug ビルド
cd editors/vscode
npm install
npm run watch               # esbuild の watch
npx tsc --noEmit            # 型検査
```

`editors/vscode` を VS Code で開き、「Run Extension (debug server)」で起動すると、`RUBY_LSP_RS_SERVER_PATH` に `target/debug/ruby-lsp-rs` を指定した Extension Development Host が立ち上がります。
release ビルドを使う場合は「Run Extension (release server)」を選んでください。
