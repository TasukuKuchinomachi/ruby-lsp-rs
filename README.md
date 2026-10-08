# ruby-lsp-rs

Rust で書いた Ruby の language server です。[Shopify Ruby LSP](https://github.com/Shopify/ruby-lsp) の移植に、RBS を使った [Steep](https://github.com/soutaro/steep) 相当の型検査を組み込んでいます。

- 型の inlay hint（メソッドの引数と戻り値、`attr_*`、変数、ブロック引数）
- hover での型表示と、RBS のシグネチャ・Ruby の実装へのリンク
- 型エラーの診断
- `ruby-lsp-rs type-check` によるプロジェクト全体の型検査

## 必要なもの

- Rust（stable）
- 対象プロジェクトの Bundler で `rbs` gem が入っていること。core と stdlib の RBS はこの gem から読み込みます
- `Steepfile` と `sig/`、`rbs_collection.lock.yaml`（あれば読み込みます）

## ビルド

```sh
cargo build --release
```

## VS Code

[`editors/vscode`](editors/vscode/README.md) を参照してください。

## CLI

```sh
cd /path/to/project
ruby-lsp-rs type-check --ruby-command "mise exec -- ruby"
```

| オプション | 内容 |
| --- | --- |
| `--ruby-command` | Bundler 環境の検出に使う Ruby の起動コマンド（既定は `ruby`） |
| `--severity-level` | `error` / `warning` / `information` / `hint`（既定は `warning`） |
| `--steep-compatible` | `steep check` と同じ診断を出す。既定では Steep より型を強く推論するため、Steep が見逃すエラーも報告します |
| パス | 検査するファイル・ディレクトリ（省略時は `Steepfile` の `check` 対象） |

診断が 1 件でもあれば終了コードは 1 です。

## リポジトリの構成

| パス | 内容 |
| --- | --- |
| `src/` | language server（[ARCHITECTURE.md](ARCHITECTURE.md)） |
| `crates/rbs-environment` | RBS・`Steepfile`・`rbs_collection.lock.yaml` の読み込み |
| `crates/type-checker` | 型推論と型検査 |
| `crates/prism-child-nodes` | Prism のノードを走査するための生成コード |
| `editors/vscode` | VS Code 拡張 |
| `tools/steep_parity` | Steep と結果を比べるツール |
