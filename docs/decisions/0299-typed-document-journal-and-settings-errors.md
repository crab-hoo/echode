# ADR 0299: 文書・ジャーナル・設定の失敗を型で保持する

- 日付: 2026-11-10
- 状態: 採用
- 対象: [未完了項目の統合](../reviews/2026-11-08-open-items-register.md) の M1。
- 関連: [ADR 0234](0234-typed-journal-durability.md)、[ADR 0296](0296-typed-storage-errors.md)、[ADR 0298](0298-typed-package-validation-errors.md)。

## 問題と再現

開始時の HEAD は `d66ec34`。
文書のロック・ワーカーの終了・復旧同意、ジャーナルの読込みと適用、設定の拒否は `PackageError::Message` に戻っていた。
一括編集の `HostError::Element` は添字・引数かのフラグ・文だけを持ち、既に型のある保存とアクションの原因も失っていた。
保存の耐久性の不確定を最初に受けた時は型で停止していたが、停止後の返信は同じ文の `Message` になっていた。

製品の `recover` と設定の書込みを通す回帰は、変更前に
`journal lost its UTF-8 cause` と `export settings lost their JSON cause` で失敗した。
既存の不確定なジャーナル書込みの故障注入を通す回帰も、後続の編集の返信で
`halted reply lost its durability kind` と失敗した。
期待終了コード 101 と固有のアサーションは `target/focused-checks/document-errors-red/` と `halted-kind-red/` にある。

一括編集の回帰は、初めに存在しない音声を I/O の失敗と想定したが、実際は AudioToolbox の `MediaError::Open(OSStatus)` だった。
原因の型に期待値を合わせた後、従来と同じく `HostError::Element::source` を切る反例で
`transaction lost its decoder cause` の失敗を照合した（`target/focused-checks/transaction-cause-red/`）。
I/O を期待していた最初の回帰は、修正の反例の証拠に数えない。

## 比べた案

| 案 | 判断 |
|---|---|
| 文と種類のフラグを並べて保持する | 採らない。原因を失い、フラグと原因が別の正本になる |
| `PackageError` の直下へ文書・ログ・設定の全変種を足す | 採らない。既存の音声・Git の種類と混ざる |
| `Document`・`Journal`・`Settings` の領域別の型を持つ | 採る。原因の連鎖と領域ごとの種類を一つの境界で保持できる |
| 一括編集に `parameters` のフラグを残す | 採らない。`ElementError` の変種から同じ規則で求める |
| 停止の理由を文か共有のエラーとして保持する | 採らない。この状態になる理由は耐久性の不確定だけで、可変の文と共有の仕組みは要らない |
| 停止を真偽値にし、毎回同じ型の不確定を返す | 採る。停止条件と返す種類が一致し、表示文も変わらない |
| 未分類の `PackageError::Message` を残す | 採らない。この作業で製品の使用箇所がなくなったので、コンストラクターごと除く |

時間の改善は目的でなく、性能の計測を判断の根拠にしない。
根拠は製品経路での原因の消失の反例と、種類・原因・文面・原本・巻戻しの照合である。

## 決定と境界

- `error_document.rs` に `DocumentError`・`JournalError`・`SettingsError` を置く。
  同意待ちは通知した `RecoveryNotice` をそのまま保持し、文にしてから復元しない。
- ジャーナルの `Apply` は連番と `ModelError`、`Invariants` は連番と `InvariantError` を持つ。
  UTF-8 と既存の `decode_line` の `ActionError` も `Error::source` へつなぐ。
  `decode_line` が内側の JSON エラーを `CorruptJournal` にする既存の契約は変えない。
- `host_error.rs` の `HostError::Element` は添字と `ElementError` を持つ。
  参照の拒否、引数の JSON、アクション、パッケージを分け、元の原因を連鎖で読める。
  RPC のコードは `ElementError::is_parameters()` から求め、文字列化は応答の境界にする。
- 文書を開く時とスレッド生成の I/O は原因を保持する。
  書出し用の私有パッケージも、不変条件の拒否と作成の I/O を型で保持する。
- 追記と同期、失敗時の巻戻し、不確定時の停止、復旧の同意、設定の既定値への後退は変えない。
  保存形式は schema 6 のままで、表示文と RPC のコード・失敗した添字も維持する。

## 回帰と基盤

- `tests/document_errors.rs` は製品ワーカー・復旧・設定の入口を通し、原因・種類・文面、
  失敗した一括編集の巻戻し、設定の 16 KiB の境界と非書換え、復旧通知と確認の一致を照合する。
- `host_tests.rs` は不確定後の編集・一括編集・Undo・Redo の拒否の型と、切断したワーカーの終了を確認する。
  文だけが不確定に似た I/O の失敗では停止しない対照も維持する。
- 既存の `audit_recovery.rs` と `parameter_ownership.rs` に連番と元の不変条件・モデルの原因の照合を足す。
  既存の一括取込みの回帰は原因の型を参照するようにする。
- `check_typed_errors.py` の移行済みファイルを広げ、文字列の結果・原因の文字列化・未分類のメッセージを拒否する。
  未移行の対照は `thin.rs` に移し、移行済みの全ファイルへの再導入の Python 回帰を維持する。
- `scripts/verification.json` のジャーナル不変条件の反例 2 件を新しい表現へ追従させる。
  必須契約の判定と適用範囲は変えない。新しい検査入口・依存・外部入力はなく、Makefile の追加は不要である。
- 構成・コードの所在・デバッグ手順と索引・監査の状態を更新する。
  利用者に残る制約は変えないので `status.md` とユーザー確認の追加は不要である。

## 残る範囲

M1 の完了ではない。次は間引き、その後に非既定の履歴スパイクの文字列エラーを扱う。
今回のエラー表現だけの変更では、保存・復元の規模、橋渡し、書出しの性能の計測はしない。
機器・アプリ・聴感の確認は実行しない。
