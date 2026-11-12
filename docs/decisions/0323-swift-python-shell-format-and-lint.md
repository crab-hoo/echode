# 0323. Swift・Python・シェルの整形と lint

- 日付: 2026-11-12
- 状態: 採用
- 関連: [XCA-18](../reviews/2026-11-02-cross-cutting-audit.md#xca-18-rust-以外の整形と-lint)、[統合表の M5](../reviews/2026-11-08-open-items-register.md#2-保守性)、
  [ADR 0278](0278-audit-backlog-order.md)（順番）、[ADR 0276](0276-priority-order-and-delegated-decisions.md)（優先順位）、
  [ADR 0269](0269-github-public-ci-scope.md)（GitHub の検査）、[ADR 0212](0212-verification-tiers.md)（検査の段階）

## 背景

Rust は `cargo fmt` と clippy で止めるが、Swift（261 ファイル、約 3.4 万行）、Python（52 ファイル、約 1.2 万行）、
シェル（21 本）には自動の整形も lint もなかった（XCA-18）。
監査の修正方針は、Xcode 同梱の `swift-format`、`ruff`、`shellcheck` を足し、最初は変更行だけを対象にすることだった。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | 監査どおり、変更行だけを検査する | 採らない |
| 2 | 全体を一度整形し、以後は全ファイルを検査する | 採る |
| 3 | 整形はせず、lint の規則だけを検査する | 採らない |
| 4 | SwiftLint など、Xcode にない Swift の道具を使う | 採らない |

- 案 1: 差分の基準と、道具の出力の行を対応づける層が要る。同じ内容でも差分の基準で合否が変わり、入力の識別値で合格を再利用する検査の仕組み（ADR 0197）と合わない。
  整形器は周りの文脈から行を決めるので、変更行だけを整形すると同じファイルに 2 つの書き方が残る。ADR 0276 は入り組んだ構造を堅牢性の損失に数える。
- 案 3: `swift-format lint` は規則の違反と整形の差を同じ出力で報告し、分けられない。整形を手で合わせる手間も残る。
- 案 4: 追加の導入と版の固定が要る。Xcode 同梱の `swift-format` で足りる。

## 決定

### 道具と版

| 対象 | 整形 | lint | 版の決め方 |
|---|---|---|---|
| Swift | swift-format | swift-format | Xcode 同梱（`xcrun swift-format`）。検査の識別値は `xcodebuild -version` と `swift --version` を既に含む |
| Python | ruff format | ruff check | ruff 0.17.4 |
| シェル | shfmt 3.14.1 | shellcheck 0.11.0 | PyPI の shfmt-py 4.2.0・shellcheck-py 0.11.0.1 が同梱する実行ファイル |

- 版とハッシュは `scripts/lint-tools.txt` に固定し、`make setup-lint`（`make setup` が呼ぶ）が `target/lint-tools` の仮想環境へ入れる。大域には入れない。
- ライセンスは ruff・shellcheck-py・shfmt-py が MIT、shfmt が BSD-3-Clause、shellcheck が GPL-3.0、swift-format が Apache-2.0。
  どれも開発用の道具で、製品にリンクも同梱もしない。いずれも GPL-3.0-or-later と両立する（PLAN §2.1）。
- 監査の方針になかった shfmt を足した。シェルに欠けていたのは整形で、必要な変更は 70 行だった。

### 規則

- 各道具の、固定した版の既定の規則を使う。Rust で clippy の既定の lint を使うのと同じで、規則の一覧を手で保守しない。版を上げる時は、新しい指摘を同じ変更で直す。
- 既定から変えるのは配置だけである。

| 対象 | 設定 | 置き場所 |
|---|---|---|
| Swift | 字下げ 4、行長 120 | `.swift-format` |
| Python | 行長 120、`target-version = "py311"`（検査基盤は Python 3.11 以降） | `ruff.toml` |
| シェル | `-i 4 -ci`（字下げ 4、`case` の字下げ） | `scripts/lint.py` |

- 行長は既存のコードの変更量で決めた（下の測定）。Rust の 100 は変えない。
- 例外は ruff の TRY004（型の誤りに `TypeError` を勧める）だけで、無効にした。
  該当の 5 か所は、読み込んだ JSON の検査の方針や設定の形の誤りを `ValueError` で拒否し、呼出し側がそれを捕まえる。`TypeError` にすると処理をすり抜ける。
- 生成した `EditAction.swift` は、生成器が `// swift-format-ignore-file` を出す。この内容は `make check-swift-actions` が生成器の出力とバイト単位で照合する。
- shellcheck は `.shellcheckrc` で、スクリプトがリポジトリの根から `source` する `scripts/*.env` をたどる。

### 検査と入口

- `check-lint`（自動、`always`、全入力）が `scripts/lint.py check` を実行する。事前確認 `lint-tools` が固定版の導入を確かめ、欠ければ `make setup-lint` を案内する。
- `make fmt` は `cargo fmt` と `scripts/lint.py format` を実行する。swift-format は 2 回かける（下の「整形の危険」）。ruff は import の並びも直す。
- `make check-portable`（GitHub の `check`）は `lint.py check --pinned-only` で、ruff・shfmt・shellcheck だけを実行する。
  ホストのランナーの Xcode は手元と版が異なり得て、コードを変えずに swift-format の結果だけが変わり得るからである。Swift は手元の `make verify` で検査する。

### 一度の整形

- 整形だけのコミット（`d79bbdf`）を分けた。swift-format・ruff format・shfmt の出力、ruff の安全な自動修正 64 件（import の並び 49、使われない `noqa` 12 など）、
  `#!` を持つのに実行権限がなかった Python スクリプト 23 本の実行権限である。
  このコミットは `.git-blame-ignore-revs` に載せた。`git config blame.ignoreRevsFile .git-blame-ignore-revs` で `git blame` が飛ばす。
- 整形に伴って直したもの:
  - 製品契約の反例 21 件の置換元・置換先を新しい改行に合わせた。入れる不具合は変えていない。
  - Swift のソースの文字列を照合していた Python のテスト 1 件を、空白に依存しない照合にした。
  - TSan の検査専用ランタイムの識別値は `scripts/tsan_runtime.py` 自身のハッシュを含むので、整形の後に `make setup-bridge` での作り直しが一度要った。

### 整形の危険

`NoParensAroundConditions` は `guard (x < plot.maxX), (endFrame > (first ?? 0))` の括弧を外し、`guard x < plot.maxX, endFrame > (first ?? 0)` にした。
次の整形は、これを `x<plot.maxX, endFrame>(first ?? 0)` というジェネリクスの呼出しとして読み直す。コンパイルできない。
全 261 ファイルで 2 回目に変わったのはこの 1 ファイルだけで、比較の順を入れ替えて直した。
`make fmt` は swift-format を 2 回かけ、1 回で収束しない書き方をすぐ表に出す。読み直しで意味が変わればビルドが止める。

### 直した指摘

- Python（手で直した 57 件）: `subprocess.run` の `check` の明示 24（どれも終了コードを読むので `check=False`）、`dict()` の呼出し 10、
  コレクション内の暗黙の文字列連結 6（括弧で囲んだ）、ループ変数を閉包が読む 5（どれも同じ反復の中で呼ぶので値を束縛した）、
  入れ子の `with` 5、集合の内包 1、使われない変数 1、TRY004 5（規則を無効にした）。
- Swift（整形の後に残った 15 件）: 末尾の閉包と閉包の引数の併用 7（ラベルで渡す）、1 行に並べた `case` 4（1 か所）、
  JSON の名前のままのプロパティ 2（`appliedSeq`・`exportSettings` にし、`CodingKeys` で JSON の名前を保つ。`Audio.swift` と同じ）、
  ブロックコメント 1、行長を超える行末コメント 1。
- シェル: `rm -rf` の変数に空の時の停止（`${var:?}`）を付けた 1 件。

## 測定

2026-11-12、Apple M4、Xcode 27.1、ruff 0.17.4、shfmt 3.14.1。整形で変わる行数（追加 / 削除）と、指摘の件数である。

| 対象 | 設定 | 変わるファイル | 追加 / 削除 |
|---|---|---|---|
| Swift | 字下げ 4、行長 100 | 248 | 14,399 / 5,878 |
| Swift | 字下げ 4、行長 120（採用） | 240 | 9,254 / 4,209 |
| Swift | 字下げ 4、行長 140 | 237 | 7,155 / 3,498 |
| Python | 行長 100 | 52 | 8,037 / 2,576 |
| Python | 行長 120（採用） | 50 | 5,274 / 2,015 |
| シェル | `-i 4` | — | 210 行 |
| シェル | `-i 4 -ci`（採用） | — | 70 行 |

- 既存の行のうち、Swift は 100 字を超えるものが 3,682 行、120 字が 1,370 行、Python は 1,270 行と 307 行だった。
- 整形前の swift-format の既定（字下げ 2、行長 100）の指摘は 37,905 件で、そのうち 29,064 件が字下げだった。
- ruff の既定の規則の指摘は 144 件、shellcheck は 9 件（8 件は `source` をたどれない注意）だった。
- `check-lint` の全体は 1.5 秒（壁時計、4 つの道具の合計。swift-format の全 Swift は 0.9 秒）。

## 理由

- 全体を一度整形する案は、一度の大きな差分の代わりに、以後の検査を基準のない全ファイルの検査にする。合否が作業ツリーの内容だけで決まる。
- 道具の既定を使い、版を固定する。同じ木はどの機械でも同じ判定になる。swift-format だけは Xcode の版に従い、その版は検査の識別値に入っている。
- ADR 0276 の順で、整形と lint は保守性に当たる。shellcheck と ruff の一部（閉包のループ変数、暗黙の連結、`rm -rf` の空の変数）は堅牢性の不具合も見つける。

## 影響

- 新しい環境では `make setup`（または `make setup-lint`）が要る。ネットワークを使う。`target/` を消すと道具も消え、事前確認が案内する。
- エージェントは `make verify` の前に `make fmt` を使う。lint の指摘はコードで直す。抑止（`# noqa`、`// swift-format-ignore`）は理由を書いた時だけ使う。
- Xcode を更新すると、コードを変えずに `check-lint` が swift-format の差で失敗し得る。`make fmt` の結果を整形だけのコミットにし、`.git-blame-ignore-revs` に足す。
- ruff・shellcheck・shfmt の版を上げる時は、`scripts/lint-tools.txt` の版とハッシュを変え、`make setup-lint`・`make fmt` の後に新しい指摘を直す。
- 製品の振る舞いは変えていない。
