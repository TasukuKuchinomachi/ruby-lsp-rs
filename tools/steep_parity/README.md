# steep_parity

ruby-lsp-rs の型検査が Steep と同じ結果を出すかを比べるツールです。

- 診断: `steep check --format=github` と `ruby-lsp-rs type-check` の出力
- 型: `steep langserver` と ruby-lsp-rs に同じ位置で hover を送り、型を比べる

## 使い方

```sh
cargo build --release
tools/steep_parity/run.sh /path/to/project
```

対象プロジェクトで `bundle exec steep` が動く必要があります。対象プロジェクトのファイルは書き換えません（Steep のキャッシュを除く）。

レポートは `tools/steep_parity/reports/<プロジェクト名>/` の `diagnostics.md` と `hover.md` に出ます。

| 環境変数 | 内容 |
| --- | --- |
| `STEEP_CHECK_OUTPUT` | 取得済みの `steep check --format=github --severity-level=hint` の出力。指定すると `steep check` を実行しない |
| `STEEP_COMPATIBLE=1` | `type-check --steep-compatible` と比べる |
| `RUBY_COMMAND` | Ruby の起動コマンド（既定は `mise exec -- ruby`） |
| `RUBY_LSP_RS_BINARY` | ruby-lsp-rs のバイナリ（既定は `target/release/ruby-lsp-rs`） |
| `SKIP_DIAGNOSTICS=1` / `SKIP_HOVER=1` | 診断・hover の比較を飛ばす |

`run.sh` の 2 番目以降の引数は `compare_hover.py` に渡ります（`--file-limit`、`--position-limit`、`--seed`、`--files` など。`--help` で一覧を表示します）。
