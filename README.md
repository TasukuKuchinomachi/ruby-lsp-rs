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

## 対応している Steep・rbs・Ruby

| Steep | rbs | Ruby |
| --- | --- | --- |
| 2.0.x | `~> 4.0`（Steep の smoke test は 4.0.2 で確認） | 3.2 以上 |
| 2.1.x | `~> 4.2`（Steep の smoke test は 4.2.0 で確認） | 3.3 以上 |

rbs と Ruby の範囲は各 Steep の gemspec のものです。

型検査は Steep のマイナーバージョンごとの違いに合わせます。どの Steep に合わせるかは、対象プロジェクトの `Gemfile.lock` にある steep のバージョンで決まります。`Gemfile.lock` に steep が無いときと、対応していないバージョンのときは、対応している最新の Steep（2.1）に合わせます（後者は警告を出します）。`ruby-type-check` では `--steep-version` で指定することもできます。

Steep のバージョンで変わるのは次の点です。

- `it`: Steep 2.0 は Ruby 3.3 の文法で読むので、`--steep-compatible` では `it` を self へのメソッド呼び出しとして扱います。Steep 2.1 では `_1` と同じブロック引数です
- `receiver == literal` の絞り込み: Steep 2.1 では、literal が receiver の型（`Symbol` など）の値になりうるとき、then 側で literal の型に絞り込みます。Steep 2.0 ではその then 側は到達しないものとして扱います
- ブロックが返す array literal: Steep 2.1 では、ブロックの返り値の型をヒントに tuple として推論し、tuple を含まないヒントは `to_ary` で変換します（rbs 4.2 の `Array#to_h` などが `Hash::_Pair[K, V]` を返すブロックを取るため）
- 省略できるブロックへの block-pass（`&:sym` など）: Steep 2.0 は `--steep-compatible` で `&:sym` をメソッドの proc ではなく `::Proc` として扱い、ブロックの型と合わないので `Ruby::BlockTypeMismatch` を出します。Steep 2.1 と既定のモードでは、必須のブロックと同じくメソッドの proc として検査します

core と stdlib の RBS は、対象プロジェクトに入っている rbs gem から読み込みます。RBS のパーサは [ruby-rbs](https://crates.io/crates/ruby-rbs) crate 0.3.0（RBS 4.0.2 相当）を使うため、RBS 4.1 で書けるようになった非 ASCII の名前は読めません。

`Steepfile` に `check "lib", inline: true` と書いたファイルは、Steep と同じく inline RBS（`# @rbs` や `#:` のコメント）から型を読み込み、型検査もします。

Ruby のコードは、対象プロジェクトの Ruby のバージョンにかかわらず、ruby-prism crate 1.9.0 の既定の文法で読みます。

## インストール

```sh
cargo install --locked --git https://github.com/TasukuKuchinomachi/ruby-lsp-rs --tag v0.2.0 ruby-lsp-rs ruby-type-check
```

リリースの tag は `v<バージョン>` です。1 つのリリースで上の表のすべての Steep に対応します。

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
| `--steep-compatible` | `steep check` と同じ診断を出す。既定でも型の関係は Steep と同じに判定しますが、Steep より型を強く推論するため、Steep が見逃すエラーも報告します |
| `--steep-version` | 合わせる Steep のバージョン（`2.0` / `2.1`）。省略時は `Gemfile.lock` の steep のバージョン |
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
