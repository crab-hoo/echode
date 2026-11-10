# ADR 0301: 履歴スパイクでも原因の型を保ち、初期化失敗後に終了できるようにする

- 日付: 2026-11-10
- 状態: 採用
- 対象: 統合表 M1、DEV-7・SPG-11・XCA-16
- 続き: [ADR 0300](0300-typed-thinning-errors.md)

## 問題と再現

開始時の HEAD は `4084b82`。
`echode-history` に残る `Result<_, String>` は、非既定の履歴スパイクの `format.rs`・`store.rs` だった。
Git・I/O・JSON・UTF-8 と、版・ファイルの欠落を文に戻し、初期化の失敗も要求ごとに文を複製していた。
読込みの 30 秒の期限は、ワーカーの終了と同じ文になっていた。

`tests/roundtrip/errors.rs` を先に追加し、既存の公開入口 `History::save`・`PendingSave::wait`・`History::load` を通した。
修正前は Git・JSON・I/O の回帰が `history spike lost its typed cause` で失敗した。
また、保存先をファイルで塞いで初期化を失敗させると、`History::drop` が終了要求を送ってもワーカーが受信待ちを続け、join が終わらなかった。
子プロセスを 10 秒で打ち切る回帰が `failed spike history initialization deadlocked during shutdown` で失敗した。
これは性能の測定ではなく、無期限の待ちの検出である。

期待終了コード 101 と固有のアサーションは `target/focused-checks/spike-errors-red/` と `spike-shutdown-red/` に記録した。
最初の fixture のコンパイル失敗は直してから再実行し、red の根拠に数えない。

## 比べた案

| 案 | 判断 |
|---|---|
| 非既定のコードなので文字列のまま除外する | 採らない。通常検査も通る公開入口で、crate 全体の再導入を防げない |
| スパイク専用の型に Git・I/O・版の欠落なども複製する | 採らない。同じ原因・操作の分類と表示を二重に保つことになる |
| `PackageError` を共用し、スパイク固有の JSON・版・ワーカーの失敗だけを feature 付きの変種へ置く | 採る。共通の原因と表示を使い、製品の既定ビルドにはスパイク固有の変種を含めない |
| 初期化失敗後にワーカーをすぐ終了する | 採らない。後続の要求から元の初期化の原因が消え、単なる終了になる |
| 初期化の原因を共有し、終了要求だけは受け付ける | 採る。後続の保存・読込みにも同じ原因を返し、破棄時の待ちを終えられる |

性能改善を目的とせず、時間・メモリの新しい計測を判断の根拠にしない。
根拠は公開入口の反例、元の原因・同一の `Arc`・従来の文面・Git の先端と保管物の照合である。

## 決定と境界

- `format.rs`・`store.rs` の結果と返信チャネルを `PackageError` にする。
  Git は既存の `GitOperation` と元の `gix::Error`、I/O は元の errno、UTF-8 と欠落は既存の変種を使う。
  オブジェクト種の変換は、従来の Debug 表示を持つ `ObjectKind` のままとする。
- `error_spike.rs` の `SpikeError` と `PackageError::Spike` は `spike` feature だけで有効にする。
  JSON はプロジェクトとトラックを分けて元の `serde_json::Error` を持ち、版の拒否は元の `u32` を持つ。
  終了と期限は型で分け、既存の `history thread stopped` という表示文は維持する。
  公開型は `echode_history::spike::SpikeError` から読める。
- 初期化失敗は `PackageError::Shared` で同じ原因を全要求へ渡す。
  失敗の返信ループも `Request::Shutdown` で抜ける。通常の保存・読込みの順番は変えない。
- Phase 0 のモデルと JSON、製品の保存形式、同期・復旧・間引き・音声経路は変えない。
  外部入力や依存・ABI を追加しない。

## 回帰と基盤

- `tests/roundtrip/errors.rs` は Git・I/O・JSON・版・UTF-8・欠落・オブジェクト種を公開入口から照合する。
  読込みの失敗後も元の版を読め、保存の失敗で先端と塞いだファイルを変えないことを確認する。
  初期化失敗の子プロセスは、保存と読込みの原因の共有と終了も確認する。
- `store_tests.rs` は返信と要求の通信路の切断、および期限と終了の変換を、待ち時間を使わず検査する。
- `check_typed_errors.py` は `echode-history` を `TYPED_CRATES` に移す。
  ファイル単位の移行の仕組みは残すが、現在の `TYPED_FILES` は空にする。
  Python 回帰は既存・新規・入れ子のモジュールと非既定 feature への文字列エラーの再導入を拒否する。
  `PackageError::new`・`Message` の拒否も、履歴の crate 全体に保つ。
- 追加回帰は既存の `check-base` の `--features spike --lib --test roundtrip` と Python 検査が通す。
  Makefile・検査の選択・必須契約・fuzz の入口は追加不要である。
- 構成・コードの所在・デバッグ手順と索引・監査の状態を更新する。
  利用者の制約は変わらず、`status.md` とユーザー確認の追加は不要である。

## 検査時の環境

最初の `make verify` は `disk-space` で停止した（必要 3.0 GiB、空き 2.6 GiB）。
検査ロックとコンパイラーの停止を確認し、今回選ばれていない ASan・TSan の再生成可能な `target/bridge/address`・`thread` だけを整理した。
空きは 4.2 GiB になり、検査記録・ユーザーデータ・通常のビルド成果物は残した。
再生成の見積もりは既存の実測の ASan 432 秒・TSan 328 秒（温まった時は 194 秒・138 秒）で、次の手元の全体検査は約 7 分余分にかかり得る。
GitHub のキャッシュは変更しない。
診断を取り直し、空き容量の回復案内とデバッグ手順に残る「ユーザーと削除を決める」という古い記述を ADR 0276 に揃えた。
検査の選択・合否基準は変えない。最初の失敗のログは `target/focused-checks/history-spike-verify.log` に保持する。

## 残る範囲

M1 全体の完了ではない。
次は `echode-ffi` の目録の境界で、型付け済みの `CatalogError` を文へ戻している所を扱う。
モデルにも `derived_order` の文字列結果と、不変条件の原因を文に戻す所が残る。
機器・アプリ操作・聴感と、保存・橋渡し・書出しの性能の計測は、この変更では行わない。
