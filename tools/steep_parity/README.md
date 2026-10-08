# steep_parity

ruby-lsp-rs の型検査が本家 Steep (2.0.0) とどれだけ同じ結果を出すかを測るハーネス。

- 診断: `steep check --format=github --severity-level=hint` と `ruby-lsp-rs check` を突き合わせる
- 型: `steep langserver` と ruby-lsp-rs の LSP に同じ位置で `textDocument/hover` を投げ、型文字列を正規化して比べる

Python 3 の標準ライブラリと、位置の収集に使う Ruby (Prism 同梱の 3.3 以降、または prism gem) だけで動く。

## 前提

- Ruby は mise で管理していて、対象プロジェクトの直下で `mise exec -- bundle exec steep ...` が動くこと
- `target/release/ruby-lsp-rs` がビルド済みであること (`cargo build --release`)。別のバイナリを使うときは `RUBY_LSP_RS_BINARY` で指定する
- ハーネスは対象プロジェクトのファイルを書き換えない (Steep が作るキャッシュを除く)

## 使い方

```sh
tools/steep_parity/run.sh <project path> [compare_hover.py に渡す追加引数...]
```

レポートは `tools/steep_parity/reports/<project 名>/` に出る (`reports/` は `.gitignore` 済み)。

環境変数:

| 変数 | 意味 |
|---|---|
| `STEEP_CHECK_OUTPUT` | 取得済みの `steep check --format=github --severity-level=hint` の出力ファイル。指定すると steep check を実行せずこれを使う |
| `RUBY_LSP_RS_BINARY` | ruby-lsp-rs のバイナリ (既定は `target/release/ruby-lsp-rs`) |
| `RUBY_COMMAND` | `ruby-lsp-rs check --ruby-command` と位置収集に使う Ruby (既定は `mise exec -- ruby`) |
| `SKIP_DIAGNOSTICS=1` | 診断の比較を飛ばす |
| `SKIP_HOVER=1` | hover の比較を飛ばす |

例:

```sh
STEEP_CHECK_OUTPUT=/path/to/steep.check.txt \
  tools/steep_parity/run.sh /path/to/project

# hover のサンプルを増やす (300 ファイル、ファイルあたり 200 位置、seed 7)
SKIP_DIAGNOSTICS=1 tools/steep_parity/run.sh /path/to/project \
  --file-limit 300 --position-limit 200 --seed 7

# 全ファイル・全位置
SKIP_DIAGNOSTICS=1 tools/steep_parity/run.sh /path/to/project \
  --file-limit 0 --position-limit 0

# 特定のファイルだけ
SKIP_DIAGNOSTICS=1 tools/steep_parity/run.sh /path/to/project \
  --files app/models/example.rb
```

`compare_hover.py` の主な引数:

| 引数 | 既定 | 意味 |
|---|---|---|
| `--file-limit` | 100 | Steepfile の check 対象からランダムに選ぶファイル数。0 で全ファイル |
| `--position-limit` | 60 | ファイルあたりの位置数の上限 (超えたらランダムに選ぶ)。0 で無制限 |
| `--seed` | 1 | ファイル・位置の抽出に使う乱数の seed |
| `--files` | なし | 抽出せずにファイルを直接指定する (プロジェクトからの相対パス) |
| `--steep-command` | `mise exec -- bundle exec steep langserver --jobs=2` | Steep の起動コマンド。langserver は裏で全ファイルを型検査するので、hover を邪魔しないよう worker を減らしている |
| `--first-hover-timeout` | 1800 | Steep の最初の hover の待ち時間 (秒) |
| `--example-limit` | 40 | レポートに載せる不一致の例の数 (種類ごと) |

## 構成

| ファイル | 役割 |
|---|---|
| `run.sh` | 下の 2 つを順に実行し、`timing.txt` に所要時間を書く |
| `compare_diagnostics.py` | 診断の比較 |
| `compare_hover.py` | hover の比較 |
| `collect_positions.rb` | Prism でメソッド呼び出し・代入・ブロック引数・def の位置を集める |
| `type_normalization.py` | 型文字列の構文解析と正規化 |
| `lsp_client.py` | 最小限の LSP クライアント |

## レポートの読み方

### `diagnostics.md` (診断)

1. **最重要指標**: Steep が error / warning で出した診断のうち、ruby-lsp-rs が出していない件数と code 別の内訳。目標は「Steep の error / warning を取りこぼさない (Steep より厳しいのは許容)」なので、まずこの数を 0 に近づける
2. **severity 別の集計**: Steep の severity (github 形式の `error` / `warning` / `notice`) ごとに、ruby-lsp-rs も出したか、出した場合に severity が一致したか。ruby-lsp-rs の `information` / `hint` は `notice` に読み替える
3. **全体**: 突き合わせの件数
   - 一致: (path, line, column, code) が同じ
   - 位置ずれ: 同じ path・line・code で column だけ違う
   - ruby-lsp-rs だけが出した: 誤検知候補
   - Steep だけが出した: ruby-lsp-rs が実装済みの code なら取りこぼし、未実装の code (ruby-lsp-rs のソースに `"Ruby::Xxx"` の文字列が無く、出力にも現れない) なら「未実装」として別に数える
4. **code 別の集計**: 実装済み / 未実装、Steep 側の severity の内訳つき
5. 位置ずれ・誤検知候補・取りこぼしの一覧 (該当ソース行つき)。未実装の code は error / warning を全件、notice を code ごとに 5 件まで
6. 一致したもののうちメッセージが違うもの (型名の `::` などを正規化して比較)
7. 除外したファイルの一覧

Steep の結果に `Ruby::UnexpectedError` が出たファイルは、Steep がそのファイルを検査できていない (RBS の再定義が collection と重複定義になっている場合など) ので、両者の診断ごと比較から除外し、件数だけ載せる。

レポートに載せるファイル内容は `.rb` / `.rbs` のソース行だけ。

`diagnostics.json` に同じ集計が機械可読な形で入る。

### `hover.md` (型)

比べる位置は次の 5 種類。

| 種類 | 位置の決め方 | Steep 側の型 | ruby-lsp-rs 側の型 |
|---|---|---|---|
| ローカル変数の代入 | ruby-lsp-rs の型 hint が出た位置 | `` **Local variable** `x: T` `` の T | `x: T` の T |
| インスタンス変数の代入 | 同上 | コードブロックの型 | `@x: T` の T |
| ブロック引数 | 同上 | コードブロックの型 | `x: T` の T |
| def の戻り値 | 同上 | `def m: (...) -> R` の R (オーバーロードがあれば `Internal method type` の R、無ければ各 R の union) | `(Receiver) → R` の R |
| メソッド呼び出し | Prism で集めた全呼び出しのメソッド名 | 最初のコードブロック (呼び出しの戻り値) | `(Receiver) → R` の R |

ruby-lsp-rs の hover に型が無い位置は型 hint のラベルを使う。

- **種類別の一致率**: 一致率 = 一致 / (一致 + 不一致)。片方の hover が無い位置は「hover なし」として別に数え、一致率に入れない。Steep が `No compatible method type found` を返した呼び出しも別に数える
- **不一致のパターン**: 「ruby-lsp-rs が untyped」「nil の有無だけ違う」「同じ総称型で型引数が違う」などに機械的に分類した件数
- **頻出する不一致の組**: 正規化後の (Steep, ruby-lsp-rs) の組の上位 30
- **不一致の例**: 種類ごとに場所・名前・両者の型・ソース行
- **hover が無い位置の例**

`hover.positions.json` に全位置の生データ (両者の hover の Markdown を含む)、`logs/` に LSP の送受信ログが入る。

def の戻り値は、Steep が RBS に書かれた型を出すのに対して ruby-lsp-rs の hint は本体から推論した型を出すことがあり、`void` と `untyped` の不一致の多くはこれによる。

### 型文字列の正規化ルール

- 型名の先頭の `::` を除去する (`::Array[::Integer]` → `Array[Integer]`)
- `A?` と `(A | nil)` と `nil | A` を同一視する (union を展開して nil を取り出し、最後に `?` で表す)
- union / intersection の要素の順序を無視する (要素を正規化した文字列でソート)。重複を除き、入れ子の union を平坦化する
- `bool` と `true | false` を同一視する
- グループ化のための括弧を除く (`(A)` → `A`)
- proc 型・メソッド型の引数名を除く (`^(Integer x) -> void` → `^(Integer) -> void`)
- proc 型の self 束縛 (`[self: T]`) は残して比べる
- 空白の違いを無視する (トークン単位で組み直す)
- Steep の論理型 (`<% Steep::AST::Types::Logic::ReceiverIsNil %>` など、`nil?` や `is_a?` の戻り値) を `bool` とみなす
- メソッド呼び出しで Steep の戻り値に残る `self` は、ruby-lsp-rs の hover が示すレシーバー型 (`(Receiver) → R` の Receiver) に置き換える
- 変数名の接頭辞 (Steep の `` **Local variable** `x: T` ``、ruby-lsp-rs の `x: T`) を除いて型だけを取り出す
- メソッドの戻り値は、Steep のメソッド型 (`def m: (...) -> R`) と ruby-lsp-rs の `(Receiver) → R` から R を取り出して比べる
- 構文解析できない型は `::` の除去と空白の正規化だけを行う

## Steep langserver の待ち方について

実際に試して次のように決めた。

- Steep の interaction worker は RBS の読み込みが終わるまで hover を処理しないので、最初の hover はそのまま返事を待つ (他の steep が同時に動いていると長くなる)。診断の publish を待つ必要はない (hover は要求ごとにそのメソッドを型検査し直す)
- interaction worker は最新の要求だけを処理し、古い要求には `null` を返す。そのため hover は 1 件ずつ、返事を待ってから次を送る
- 位置がトークンの先頭ちょうどだと、Steep は直前のノード (例: `foo[:x]` の `[` で `foo`) を返すことがある。そのため名前の 2 文字目を指す

## 既知の制限

- 演算子・添字呼び出し (`+`, `[]`, `==` など) は hover の比較から外している (Steep が隣のノードを返すため)。件数はレポートに載る
- `method(key:)` の省略記法の `key` は、Steep の hover がシンボルを返すので対象外
- `rescue => e` の `e` など、Steep が hover を返さない位置がある (「Steep の hover なし」に数える)
- def の戻り値は Steep が RBS の宣言を、ruby-lsp-rs が推論結果を出すことがあり、厳密には同じものを比べていない
- Steepfile は `check` / `ignore` の文字列だけを読む簡易解析。target が複数ある場合も区別しない
- 「未実装」の判定は ruby-lsp-rs のソース中の `"Ruby::Xxx"` という文字列の有無による
- 位置ずれは同じ行・同じ code のものだけを対応付ける。行がずれたものは誤検知と取りこぼしの両方に数えられる
- UTF-16 で表せない文字 (絵文字など) を含む行では、Steep 側の column の解釈がずれる可能性がある
