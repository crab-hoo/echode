# 0325. Swift のテストの期限を、イベント列の外側の監視で掛ける

- 日付: 2026-11-12
- 状態: 採用
- 関連: [XCA-09](../reviews/2026-11-02-cross-cutting-audit.md#xca-09-避けられる高コストの罠)、[統合表の M8](../reviews/2026-11-08-open-items-register.md#2-保守性)、
  [ADR 0233](0233-verification-deferral-reasons-and-traps.md)（残る範囲）、[ADR 0210](0210-sanitizer-test-width.md)（同時実行の幅）、
  [ADR 0278](0278-audit-backlog-order.md)（順番）、[ADR 0276](0276-priority-order-and-delegated-decisions.md)（優先順位）
- 置き換え: ADR 0233 の「Swift のテストの `.timeLimit` は残す」

## 背景

開始時の HEAD は `5e3bc4a`。

XCA-09 の残りは、Swift のテストに期限がなく、1 件の待ち続けで検査全体の期限（`check-swift` は 1,200 秒）まで止まることだった。
2026-10-25 には、再開されない `await` で約 4 時間止まり、期限の後に `swiftpm-testing-helper` が親を失って残った。
ADR 0233 は、`.timeLimit` をまとめて付ける `@Suite` がないことを理由に残していた。

この時点の Swift のテストは次のとおりだった。

- `EchodeAppTests`: Swift Testing の 325 件（121 スイート）。`check-swift`、製品契約、ASan・TSan、`check-real-hal` が動かす。
- `EchodeBridgeTests`: XCTest の 16 件（6 ファイル）。`check-swift` が動かす。
- `spikes/input` の `EchodeInputCoreTests`: XCTest の 12 件（1 ファイル）。`check-input` が動かす。

## 測定

`.timeLimit(.minutes(1))` を付けた 3 種の待ちを、作業用のパッケージで 1 件ずつ動かした（外側に 150 秒の `alarm`）。

| 待ちの形 | 結果 |
|---|---|
| `Task.sleep`（取消に応じる） | 64.0 秒で `Time limit was exceeded` の失敗として終わった |
| 再開されない `withCheckedContinuation` | 150 秒の `alarm` まで終わらず、`swiftpm-testing-helper` が PPID 1 で残った。`-q` の出力は空だった |
| `DispatchSemaphore.wait()`（スレッドを塞ぐ） | 同じく 150 秒まで終わらず、ヘルパーが残った |

Swift Testing の期限はテストのタスクを取り消すだけで、取消に応じない本体の終わりを待つ。2026-10-25 の形（再開されない `await`）は止まらない。
既定の期限を全テストへ与える公開の手段もない（`__CommandLineArguments_v0` と `SWT_*` の環境変数に該当がない。`Configuration` のものは SPI）。

健全な実行のイベント列を測った（`--event-stream-version 0`）。

| 実行 | 幅 | 全体 | 記録の最大の間隔 | 最長のテスト本体 |
|---|---|---|---|---|
| `check-swift` の `EchodeAppTests`（2 回） | 無制限 | 24.0 秒 | 2.2 秒 | 全体とほぼ同じ |
| ASan（`check-bridge-asan`） | 16 | 22.9 秒 | 0.6 秒 | 7.9 秒 |
| TSan（`check-bridge-tsan`） | 16 | 25.3 秒 | 0.7 秒 | 11.0 秒 |
| TSan のテストをまたぐ競合の実行 | 無制限 | 32.2 秒 | 3.1 秒 | 全体とほぼ同じ |

幅が無制限の実行では、全 325 件が最初の 0.03 秒で始まり、主アクターを交互に使って全体の終わり近くで終わる。
テストごとの経過時間は、そのテストの仕事ではなく実行全体の長さを測る。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | 全テスト（または全スイート）に `.timeLimit` を付け、付け忘れを検査で止める | 採らない |
| 2 | 外側からテストごとの経過時間に上限を掛ける | 採らない |
| 3 | 外側からイベント列を読み、記録が一定時間途絶えたら、走っているテストを名指しして止める | 採る |

- 案 1: 測定のとおり、取消に応じない待ちを止めない。止められる形でも、`-q` の出力は止まった実行とともに失われる。
  最小の単位は 1 分で、325 件への付与と、XCTest への別の仕組みが要る。
- 案 2: 幅が無制限の実行では、上限は実行全体の長さに掛かる。忙しいマシンで全体が延びると、健全なテストを止める。
  誤検出を避けるには上限を全体（24〜32 秒）より十分大きくする必要があり、止まるまでが遅い。
- 案 3: 健全な実行は、どの幅でも 3.1 秒より長く記録が途絶えない。全体が延びても、テストが終わり続ける限り止めない。
  取消に応じない待ちも、外から子プロセスごと止める。同じ作業用のパッケージで、10 秒の上限に対して両方の待ちを 13〜14 秒で止め、名前を示し、プロセスを残さなかった。

## 決定

- `scripts/swift_test_watchdog.py` を全ての Swift のテストの実行に通す。
  - 実行は `--event-stream-output-path` と `--event-stream-version 0` を持つ。監視はその列を読み、最初の記録から後に 120 秒（`STALL_SECONDS`）新しい記録がなければ止める。
    最初の記録の前（ビルドとテストの探索）は数えない。その間は各検査の期限が止める。
  - 止める時は、始まって終わっていないテスト（パラメーター化したテストは各ケース）を古い順に名前と経過秒で示す。
    `swiftpm-testing-helper`（なければ直接の子）の `sample` を `<列>.stall.txt` に残し、子孫のプロセスを全て止める。
    コマンドとしての監視は 124 で終わる。プロセス内の監視では、止めた実行の終了コードのまま、報告を出力の末尾に足す。
  - 実行は呼び出し側のプロセスグループに残す。検査の外側の期限は従来どおり全体へ届く。
- 120 秒は、測った最大の間隔 3.1 秒の約 40 倍、最長のテスト本体 11.0 秒の約 11 倍である。忙しいマシンで 2〜3 倍遅くなっても届かない。
  健全な実行が止められたら、その列の間隔を測り直して変え、理由をこの ADR に追記する。
- 生きているテストの数え方は `check_test_width.py` と共有する（`LiveTests`）。幅の検査と監視が別の規則で数えない。
- 入口ごとの掛け方:
  - シェルと Makefile（`check-shell.sh`、`check-input`、`coverage.sh`）と `check_real_hal.py` は、コマンドの前に `swift_test_watchdog.py --` を置く。
  - `run-bridge-validation.py`（ASan・TSan）と `check_product_contracts.py`（契約）は、同じ `Watchdog` をプロセス内のスレッドで動かす。
    間に入るインタープリターが SIP の `/usr/bin/python3` だと `DYLD_*` が落ち、固定した TSan のランタイムや Testing のフレームワークが外れるからである。
  - 契約は契約ごと（まとめた実行はバンドルごと）に列を持つ。止まった契約は、その契約の失敗として名前付きで記録される。
    従来は 1 件の期限（180 秒）か、まとめた実行の期限の合計まで待ち、検査の中では検査全体を止めていた。
- XCTest はイベント列を書かないので、監視から見えない。`EchodeBridgeTests` の 16 件と `EchodeInputCoreTests` の 12 件を Swift Testing へ移した。
  `accuracy:` の比較は、テスト内の補助関数にした。`EchodeBridgeTests` は同時に動くようになり、16 件で 0.3 秒だった。
- `check-verification` の `test_swift_test_watchdog.py` が次を止める。
  - 監視を通らない `swift test`（Makefile と `scripts/*.sh`）、プロセス内の監視の削除、テストのターゲットでの `import XCTest`。
  - 止まった実行を止めないこと、名前の誤り、子孫の取り残し、健全な実行の誤検出、最初の記録の前を数えること、前の実行の列の読み残し。
- `test_tsan_runtime.py` の製品の橋渡しの検査の複製で、ASan の幅 16 の実行と TSan の競合の実行の待ち続けが、名前付きの失敗で終わることを確かめる。

## 残る範囲

- 最初の記録の前の待ち（ビルド、テストの探索、静的な初期化）は、従来どおり各検査の期限で止まる。
- ほかのテストが終わり続けている間の待ち続けは、それらが終わってから 120 秒後に止まる。今の実行は 25〜32 秒で終わる。
- 再開されない `await` の `sample` は、どのスレッドにもテストの枠が出ない。この形は名前で探す（[デバッグ手順](../debugging/verification-and-build.md)）。

## 影響

- XCA-09 と統合表の M8 を閉じた。ADR 0233 の残る範囲の記述は、当時の記録として残す。
- 検査の選択では、`swift_test_watchdog.py` を Swift のテストを動かす全ての検査の入力に足した。
