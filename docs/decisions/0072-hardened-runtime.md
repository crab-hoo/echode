# ADR 0072: 本体の署名は Hardened Runtime を明示する

- 日付: 2026-10-13
- 状態: 採用
- 範囲: `Echode Local` で署名する配布バンドル。公証は [ADR 0061](0061-distribution-signing.md) のまま Phase 6

## 背景

`project.yml` は Hardened Runtime が無効で、署名コマンドにも `--options runtime` が無かった（監査 AUD-13）。ヘルパーだけが library validation を外す権限を持っていても、本体側でその検査が有効だとは限らない。Apple の説明では、Hardened Runtime を付けると library validation が既定で有効になる。

## 決定

`Echode Local` による署名は `--options runtime` を付ける。Xcode の設定も Hardened Runtime を有効にする。本体の権限には `disable-library-validation` を入れない。ヘルパーだけが、これまでどおりその権限を持つ。

証明書が無いときの `make app` のアドホック署名には runtime を付けない。アドホックのままだと、本体が自分のフレームワークを拒む。公証はしない。

## 結果

一時証明書で署名した本体とヘルパーの署名フラグは `runtime` である。別のアドホック署名の動的ライブラリは、本体の `dlopen` が失敗し、ヘルパーは読み込める。画面での操作確認は未実施である。
