# ADR 0303: モデルの並びと不変条件の原因を型で保つ

- 日付: 2026-11-10
- 状態: 採用
- 関連: [ADR 0276](0276-priority-order-and-delegated-decisions.md)、[ADR 0298](0298-typed-package-validation-errors.md)、[ADR 0299](0299-typed-document-journal-and-settings-errors.md)
- 対象: 統合表 M1（DEV-7・SPG-11・XCA-16）のモデルの境界

## 問題と再現

開始時の HEAD は `a3f41a5`。
`derived_order` は `Result<_, String>`、並びの照合は `Option<String>` を返していた。
`InvariantError` も文だけの構造体で、組立てはその文を `ModelError::Invariant(String)` へ写していた。
パッケージとジャーナルが元のモデルの型を持っても、その先の規則・ID・時刻を調べられなかった。

製品と同じ `Project::try_assemble` へ、自動クリップの広すぎる範囲と不正な順序キーを渡す回帰を先に足した。
旧実装は `model assembly lost its typed invariant cause` と `model assembly lost its typed arrange order cause` で失敗した。
期待終了コード 101 と固有のアサーションは `target/focused-checks/model-errors-red/` に記録した。

## 比べた案

| 案 | 判断 |
|---|---|
| 文を持つ `InvariantError` をそのまま包む | 採らない。規則を分けるには引き続き文の解析が要る |
| 各境界に原因の種類を写し直す | 採らない。モデル・保存・復旧で同じ分類を重複して持つ |
| 規則を列挙型にし、元の原因を包む | 採る。既存の検査を一つの正本のまま保てる |
| 並びの原因もすべて値で内包する | 採らない。後段の `DocumentError::History` が 128 バイト以上になり `result_large_err` が止めた |
| 並びの原因だけを `Box` にする | 採る。失敗時だけの確保で、規則・対象 ID・元の表示を失わず後段の値を小さく保つ |

Apple Silicon の現在のビルドで `size_of` を調べた結果は、`ModelError` 64、`InvariantError` 64、`ArrangeError` 80 バイトだった。
測定用バイナリは `target/focused-checks/model-error-layout` に置いた。
時間・音質・有効なプロジェクトの使用メモリの改善は主張しない。

## 決定と境界

- `error.rs` の `InvariantError` は、規則・対象 ID・自動配置の範囲を変種と値で保持する。
  従来の文は `Display` だけが作る。浮動小数点値やスナップショットそのものはエラーへ複製しない。
- `derived_order` と並びの照合は `ArrangeError` を返す。
  キーの不正・重複・到達不能・循環・入力の並び違いを分け、ID と失敗時点の表示名を保持する。
- `ModelError::Invariant` は元の `InvariantError` を持つ。
  `ModelError::Order` と `InvariantError::Order` は `Box<ArrangeError>` を持ち、`Error::source` は箱の内側を返す。
  `ModelError::Order` の表示は従来の `BadOrder` と同じ総称の文で、詳しい原因の表示は `source` から得る。
- `BadOrder` は、編集時のキーの読込みと二つのキーの間へ挿せない拒否に残す。
  森の検査・並びの導出・不変条件の述語と検査の順は変えず、型付けのための検証を追加しない。
- 保存の形式・復旧同意・失敗時の原本保持・RPC と C ABI・有効なスナップショットの処理は変えない。
  エラーは制御側だけで作り、音声コールバックへ確保や検査を足さない。

## 回帰と基盤

- `invariants_tests.rs` の既存の文による照合のうち、組立ての原因を型の照合へ変えた。
  新しい `invariants_error_tests.rs` は同じ fixture で、組立てと直接の検査の原因・時刻、同名トラックの ID、階層、並びの原因と `source` を照合する。
- 履歴の `tests/package_errors.rs` は、実ファイルを読む製品入口で並び・自動範囲の原因と表示、拒否後の原本保持を照合する。
  `tests/audit_recovery.rs` は、不正なジャーナルを拒否した原因から配置・トラック ID を読めることを確かめる。
  `tests/track_changesets.rs` の旧 `BadOrder` の期待も、到達不能・キー不正・重複の原因と ID の照合へ変えた。
- `check_typed_errors.py` の `TYPED_CRATES` に `echode-model` を登録する。
  Python 回帰は、不変条件・モデル・新規・入れ子のモジュールへの文字列の結果と原因の文字列化の再導入を拒否する。
- 領域の設計書・コードの所在・既存のデバッグの節・監査の統合表を更新する。
  既存の通常検査と fuzz が同じ入口を通り、新しい外部入力・実行入口・必須契約はないため、Makefile と検査選択の追加は不要である。
- 利用者の操作・音・画面の制約は変わらず、`status.md` とユーザー確認の追加は不要である。

## 残る範囲

M1 全体の完了とはしない。
次は `echode-api` の `ApiError` で、I/O・パッケージの原因を文へ戻す所である。
CLI の文だけのエラー、アクションの引数エラーの表示文の解析も残る。
検査の合否と各計測・実機の未実行の理由は `target/verification/report.md` を正本にする。
