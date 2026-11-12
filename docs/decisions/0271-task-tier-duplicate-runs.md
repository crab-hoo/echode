# 0271. 作業の段階で同じテストを何度も実行しない

- 日付: 2026-11-05
- 状態: 採用
- 関連: [ADR 0148](0148-verification-pipeline-speed.md)、[ADR 0209](0209-real-hal-guard.md)、[ADR 0212](0212-verification-tiers.md)、[ADR 0269](0269-github-public-ci-scope.md)
- 置き換え: ADR 0148 の「契約のテストを 1 プロセスにまとめない」を作業の段階について。ADR 0209 の毎回の直列の全体実行
- 後続: [ADR 0327](0327-instrumented-builds-only-on-github.md) が、RTSan と変えた契約の反例を作業の段階から GitHub の `full` へ移した。

## 背景

ユーザーから、毎回走る検査に毎回の意味があるかを見直すよう依頼された。
2026-10-31 から 11-05 の `timing-history.jsonl`（`make verify` 231 回、合計 26.9 時間）で、重いのは同じテストの重複だった。

| 検査 | 実行回数 | 中央値 | 合計 | 重複の中身 |
| --- | --- | --- | --- | --- |
| `check-product-contracts` | 73 | 253 秒 | 7.9 時間 | `check` と `check-swift` が実行した必須テスト 256 件を、1 件 1 プロセスで再実行 |
| `check-verification` | 123 | 80 秒 | 2.9 時間 | どのファイルの変更でも、`scripts/tests` の全モジュールを直列に実行 |
| `check-real-hal` | 60 | 46 秒 | 0.9 時間 | `check-swift` が実行した EchodeApp のテスト全件を、直列で再実行 |

ユーザーが挙げた RTSan・入力の単体テスト・橋渡しの全 feature・変えた契約の反例は、領域が変わった時だけ選ばれ、合計 1.3 時間だった。
RTSan は音声スレッドの禁止操作を見つける唯一の検査で 5 回失敗しており、変えた契約の反例は直したテストの検出力を早く示す。どちらも残した。

## 決定

### 契約の通常テストをバイナリごとにまとめる

- 作業の段階（`--changed-controls`）では、同じテストのバイナリの契約を 1 プロセスで実行する。
  Rust は `cargo test -p <package> <target> -- <名前>... --exact`、Swift は Testing ヘルパーに `--filter` を契約の数だけ渡す。
- 証拠は、そのテスト自身の合格の行が出力に 1 回だけあることと、実行が要約の行まで進んだことである。
  無視・スキップ・失敗・既知の問題・欠落・二重の一致は証拠にしない。
- まとめた実行で示せなかった契約は、従来どおり単独で、各契約の期限のもとで実行し直す。まとめた実行の期限は、含む契約の期限の和である。
- 記録には `isolation: "batch"` を付ける。全体の段階はこの記録を再利用せず、1 契約 1 プロセスで実行する。
  全体の段階は GitHub の `full` が `main` へのプッシュごとに実行する（ADR 0269）。
- 契約が 1 件だけのバイナリは単独のまま実行する。2026-11-05 の契約 256 件では、まとめた実行 15 回と単独 36 回の 51 プロセスになった。

### HAL の検出を `check-swift` の実行に移す

- `check-shell.sh` は、EchodeApp のテストのログに代役の報告 `Real HAL reached:` があれば、テストが合格していても `check-swift` を失敗にする。
  代役は報告を標準エラーへも書くので、どのテストにも属さないスレッドからの呼出しも残る。
- `check-real-hal` は、束縛の検査（`nm -u`）と、代役が対照を名指しで止めることだけを確かめる。
- テストの名前を出す直列の全体実行は、`make check-real-hal-names`（`check_real_hal.py --name-tests`）で必要な時に行う。

### 基盤テストのモジュールを同時に実行する

- `scripts/run_unit_tests.py` が、`scripts/tests` の各モジュールを別プロセスで同時に実行する（最大 8）。
  失敗・取込みの誤り・0 件のモジュールは、その出力全体を表示して失敗にする。
- `check-verification` と `check-portable`（GitHub の `check`）の両方がこれを使う。

## 測定

MacBook Air M4、2026-11-05、ビルド済みの状態。

- 契約 256 件の通常テスト: 1 件 1 プロセスでは Rust 110 件が計 197.6 秒、Swift 146 件が計 65.0 秒だった。
  まとめると Rust が 46.9 秒（110 件すべてに合格の行）、Swift が 19.5 秒（146 件すべてに合格の行）だった。
  別のセッションの検査と並んだ実行では 318 秒かかった。負荷が違うので、この値は比べない。
- `check-real-hal`: 束縛の検査と対照だけで 8.5 秒。
- `check-shell.sh` に `ECHODE_REAL_HAL_CONTROL=1` を付けて対照のテストを動かすと、終了コード 1 で `make check-real-hal-names` を案内して止まった。
  付けない時は合格した。
- `scripts/tests` 381 件: 直列ではモジュールの合計が 90 秒、同時の実行では 32.4 秒だった。同時の実行は 3 回続けて全件合格した。

## 見送ったもの

- 契約の通常テストを、テストの一覧（`--list`）だけで確かめる案。
  Swift は `.enabled(if:)` で実行時に飛ばすテストを一覧で見分けられない。
  `check` の `cargo test --workspace` は feature の構成が `-p` と違う（ADR 0148）。別の検査の証拠に結び付けることにもなる。
- `check-verification` を、文書だけの変更では文書を読むテストに絞る案。
  入力は全ファイル（`**`）で、381 件のどれが文書を読むかの分類は漏れやすい。同時の実行で、文書だけの変更も 83 秒から約 32 秒になる。
- 作業の段階で選ばれる ASan・TSan・Miri（`unsafe` の変更時）を手元から外す案。ユーザーが今回の範囲から外した。

## 結果と制約

- まとめた実行では、同じバイナリのテストが同時に動く。
  同時の時だけ落ちて単独で通るテストは、作業の段階を通る。同時に全件を動かす `check` と `check-swift` は、引き続きそれを見る。
- 1 件ずつの隔離と各期限は、全体の段階（GitHub の `full`）で保つ。
- 実 HAL に触れたテストの名前は、毎回は出ない。`check-swift` が止めた時に `make check-real-hal-names` を実行する。
- 基盤テストのモジュールは同時に動く。機械全体の資源を占めるテストを足すと、負荷で不安定になるおそれがある。

## 回帰・検査基盤

- `test_a_batch_proves_only_a_test_with_its_own_single_pass`、`test_a_task_runs_one_process_per_binary_and_retries_what_it_did_not_prove_alone`、
  `test_a_swift_batch_builds_once_and_filters_every_test_in_one_helper_run`、`test_the_full_tier_does_not_reuse_a_tasks_batch_evidence` が、まとめた実行を検査する。
- `test_check_swift_scans_its_own_run_for_the_guard_marker` が、代役の報告の文字列と `check-shell.sh` の照合、`make check-real-hal-names` を検査する。
- `test_run_unit_tests.py` が、失敗・取込みの誤り・0 件のモジュールでの失敗と、Makefile での使用を検査する。
