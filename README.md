# ruby-lsp-rs

Rust で書いた Ruby の language server です。[Shopify Ruby LSP](https://github.com/Shopify/ruby-lsp) の移植に、[Steep](https://github.com/soutaro/steep) を Rust に移した型検査を組み込んでいます。型検査の結果は Steep と同じになるように作っており、Steep の language server が持つ機能はその形のまま返します。

- 型エラーの診断（`.rb`、inline RBS、`.rbs`。`.rbs` の構文エラーは保存前のテキストから出します）
- hover での型とメソッドの表示
- レシーバーの型からのメソッドの補完（書きかけのコードを含む）と、RBS ファイルでの型名の補完
- signature help
- 定義・宣言・型定義・実装へのジャンプ（RBS ファイルの中も）
- 型の inlay hint（メソッドの引数と戻り値、`attr_*`、変数、ブロック引数）
- `steep-check` によるプロジェクト全体の型検査（`steep check` と同じ診断を出します）

## 必要なもの

- Rust（stable）
- rbs 4.2.0 の gem。core と stdlib の RBS はこの gem から読み込みます。対象プロジェクトの `Gemfile.lock` にあるもの、対象プロジェクトの Ruby の gem のディレクトリにあるもの、`RBS_GEM_DIRECTORY` で指定したディレクトリの順に探します
- `Steepfile`（無いときは `check "."` と `signature "sig"` の 1 つの target として扱います）、`sig/`、`rbs_collection.lock.yaml`（あれば読み込みます）

## 対応している Steep・rbs

| Steep | rbs |
| --- | --- |
| 2.0.0 | 4.2.0 |
| 2.1.0 | 4.2.0 |

どの Steep に合わせるかは、対象プロジェクトの `Gemfile.lock` にある steep のバージョンで決まります。`Gemfile.lock` に steep が無いときは、gem のディレクトリにある最も新しい steep に、それも無いときは Steep 2.1.0 に合わせます。対応していないバージョンのときは警告を出して Steep 2.1.0 に合わせます。

`Steepfile` は Ruby を実行せずに読みます。決まった書き方（`target`、`check`、`signature`、`ignore`、`library`、`repo_path`、`collection_config`、`configure_code_diagnostics`、`group` など）以外の `Steepfile` は読めず、読み込みのエラーとしてログに出します。

## インストール

```sh
cargo install --locked --git https://github.com/TasukuKuchinomachi/ruby-lsp-rs --tag v0.2.0 ruby-lsp-rs steep
```

`ruby-lsp-rs`（language server）と `steep-check`（CLI）が入ります。リリースの tag は `v<バージョン>` です。

## ビルド

```sh
cargo build --release
```

`target/release/ruby-lsp-rs` と `target/release/steep-check` ができます。

## VS Code

[`editors/vscode`](editors/vscode/README.md) を参照してください。

## 設定

`initializationOptions` と `workspace/didChangeConfiguration` の `typeChecker` で設定します。

| 設定 | 既定値 | 内容 |
| --- | --- | --- |
| `enabled` | `true` | 型検査を使う機能すべて |
| `diagnostics.enabled` | `true` | 型エラーの診断 |
| `definition.target` | `"implementation"` | 定義へ移動の飛び先。`"signature"` で RBS、`"both"` で両方 |
| `inlayHints.methodReturnTypes` など | `true` | 種類ごとの型の inlay hint |
| `inlayHints.hideObviousTypes` | `true` | `Foo.new` やリテラルのように型が明らかなものは表示しない |
| `inlayHints.hideUntyped` | `true` | `untyped` は表示しない |
| `inlayHints.maxLength` | `30` | hint の最大長（`null` で無制限） |

## CLI

```sh
cd /path/to/project
steep-check check
```

`steep check` と同じ診断を出します。オプションは `steep-check --help` を参照してください。

## リポジトリの構成

| パス | 内容 |
| --- | --- |
| `src/` | language server（[ARCHITECTURE.md](ARCHITECTURE.md)） |
| `crates/rbs-definition` | rbs gem の `Environment`・`AncestorBuilder`・`DefinitionBuilder` と、RBS・inline RBS・rbs collection の読み込み |
| `crates/steep-interface` | Steep の型、`Interface::Builder` の shape、`Subtyping::Check` |
| `crates/steep` | `Steepfile`、parser gem の形の AST、`TypeConstruction`、診断、Steep の services（`TypeCheckService`、`HoverProvider`、`CompletionProvider`、`SignatureHelpProvider`、`GotoService`）、`steep-check` |
| `crates/smoke-test-runner` | Steep の smoke test と `e2e/cases` を `steep-check` で走らせて集計する |
| `crates/prism-child-nodes` | Prism のノードを走査するための生成コード |
| `crates/ruby-environment` | Bundler と RubyGems から Ruby 環境を調べる |
| `crates/ruby-file-glob` | Ruby の `File.fnmatch` と `Dir.glob` 相当 |
| `e2e/` | `steep check` の期待値を持つ end-to-end のケース |
| `fixtures/` | rbs gem・Steep と層ごとに比べるための入力 |
| `tools/run_smoke_tests.sh` | Steep の smoke test と `e2e/cases` を走らせる（`--steep-version=2.0.0` / `2.1.0`） |
| `tools/compare_with_rbs_and_steep.sh` | environment・ancestors・definitions・shapes・subtyping・parser・hover・completion・signature help・goto を rbs gem と Steep の出力と比べる |
| `editors/vscode` | VS Code 拡張 |
