# ruby-lsp-rs

Rust で書いた Ruby の language server です。[Shopify Ruby LSP](https://github.com/Shopify/ruby-lsp) の移植に、RBS を使った [Steep](https://github.com/soutaro/steep) 相当の型検査を組み込んでいます。

- 型の inlay hint（メソッドの引数と戻り値、`attr_*`、変数、ブロック引数）
- hover での型表示と、RBS のシグネチャ・Ruby の実装へのリンク
- 型エラーの診断
- `ruby-type-check` によるプロジェクト全体の型検査（`steep check` の代わりに CI で使えます）

## 必要なもの

- Rust（stable）
- 対象プロジェクトの Bundler で `rbs` gem が入っていること。core と stdlib の RBS はこの gem から読み込みます
- `Steepfile` と `sig/`、`rbs_collection.lock.yaml`（あれば読み込みます）

Steep 2.1.0 と rbs 4.2.0 に合わせています。RBS のパーサは [ruby-rbs](https://crates.io/crates/ruby-rbs) crate 0.3.0（RBS 4.0.2 相当）を使うため、RBS 4.1 で書けるようになった非 ASCII の名前は読めません。

## ビルド

```sh
cargo build --release
```

`target/release/ruby-lsp-rs`（language server）と `target/release/ruby-type-check`（CLI）ができます。

## VS Code

[`editors/vscode`](editors/vscode/README.md) を参照してください。

## CLI

```sh
cd /path/to/project
ruby-type-check --ruby-command "mise exec -- ruby"
```

| オプション | 内容 |
| --- | --- |
| `--ruby-command` | Bundler 環境の検出に使う Ruby の起動コマンド（既定は `ruby`） |
| `--severity-level` | `error` / `warning` / `information` / `hint`（既定は `warning`） |
| `--steep-compatible` | `steep check` と同じ診断を出す。既定では Steep より型を強く推論するため、Steep が見逃すエラーも報告します |
| `--format` | `pretty`（既定。rustc のようにソースの該当箇所を示す）/ `line`（1 診断 1 行） |
| パス | 検査するファイル・ディレクトリ（省略時は `Steepfile` の `check` 対象） |

```
error[Ruby::NoMethod]: Type `User` does not have method `unknown_method`
  --> lib/user.rb:15:6
   |
15 | user.unknown_method
   |      ^^^^^^^^^^^^^^
```

診断が 1 件でもあれば終了コードは 1 です。端末に出力するときは色が付きます（`NO_COLOR` を設定すると付きません）。

## リポジトリの構成

| パス | 内容 |
| --- | --- |
| `src/` | language server（[ARCHITECTURE.md](ARCHITECTURE.md)） |
| `crates/ruby-type-check` | `ruby-type-check` CLI。`Steepfile` の対象ファイルを集めてプロジェクト全体を検査する |
| `crates/rbs-environment` | RBS・`Steepfile`・`rbs_collection.lock.yaml` の読み込み |
| `crates/type-checker` | 型推論と型検査 |
| `crates/prism-child-nodes` | Prism のノードを走査するための生成コード |
| `crates/ruby-environment` | Bundler と RubyGems から Ruby 環境を調べる |
| `crates/ruby-file-glob` | Ruby の `File.fnmatch` と `Dir.glob` 相当 |
| `editors/vscode` | VS Code 拡張 |
| `tools/steep_parity` | Steep と結果を比べるツール |
