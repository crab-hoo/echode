# ADR 0332: 通常の check-swift も同時実行数 16 で合否を決める

日付: 2026-11-13
状態: 採用
関連: [ADR 0210](0210-sanitizer-test-width.md)、[ADR 0185](0185-piano-auto-and-fixed-grid.md)、[ADR 0325](0325-swift-test-stall-watchdog.md)、[ADR 0331](0331-off-worker-audio-import.md)

## 問題と再現

ADR 0331 の作業の `make verify` で、`check-swift` の `RecoveryConsentTests.contractRecoveryWarningPrecedesDocumentPublicationAndEditing`（必須契約 `recovery-consent-ui`）が 1 回失敗した。
最初の文書を開く時の 5 秒の待ちが切れ、その時の `busy` は真だった（開く処理の途中）。同じスイートの単独実行は 3 回とも 0.29 秒で合格し、次の `make verify` も合格した。

ADR 0210 は、主アクターのテストが 1 本の列を順に使うので、全テストが同時に生きると 1 回の `await` ごとに全テストの一巡を待つことを示した。
そこでサニタイザの合否を決める実行だけを同時実行数 16 にし、通常の `check-swift` は全テスト同時のまま残っていた。
イベント列では、通常の実行は 354 本の本体が同時に生きていた。

同じビルドで、全テスト同時と幅 16 を交互に 3 回ずつ動かした（Apple M4、macOS 27.0、ほかの処理なし）。

| 幅 | 全体の所要 | 復旧の契約 1 件の所要 | 合否 |
|---|---|---|---|
| 全テスト同時 | 23.9〜24.2 秒 | 19.75〜21.23 秒 | 3 回とも合格 |
| 16 | 13.3〜22.7 秒 | 1.11〜1.60 秒 | 3 回とも合格 |

失敗した回は全体 23.6 秒、復旧の契約は 10.35 秒で切れた。合格した `make verify` の回も、その契約は 20.4 秒かかった。
契約の中の 3 回の待ちは、それぞれ 5 秒の窓の大半を使っていた。

## 比べた案

1. **5 秒の期限を延ばす。** ADR 0210 の案 A と同じで、遅い応答や失われた応答を隠す。[デバッグ手順](../debugging/verification-and-build.md) も期限の延長で合格にしないとしている。採らない。
2. **復旧のスイートだけを別プロセスへ分ける（サニタイザの実行と同じ）。** この契約は直るが、ほかの 5 秒の待ち（ミキサー、デバイスの一覧など）は同じ占有に残る。採らない。
3. **合否を決める全ての Swift の全体実行を、同じ幅 16 にする。** 通常の実行には競合を探す目的がないので、全テスト同時の実行を残す理由がない。所要は延びない。採る。

## 決定

- 幅の値は `scripts/swift-test-width.env` の `ECHODE_SWIFT_TEST_WIDTH` 1 か所に置く。`check-shell.sh`（`check-swift`）と `check-product-bridge.sh`（ASan・TSan）が読む。
- `check-shell.sh` は呼出し元の環境の値を消し、EchodeBridge と EchodeApp のテストをこの幅で動かす。
  実際の幅は、実行の後に `check_test_width.py --at-most` がイベント列で確かめる。超えたら失敗にする（ADR 0210 と同じ）。
- テストの対象、5 秒の期限、全テスト同時で競合を探す TSan の実行は変えない。

## 検査

- `test_check_test_width.py::test_every_pass_or_fail_run_uses_the_one_shared_width`: 3 つの検査が同じ値を読み、環境の値を消し、幅を確かめ、検査の入力に幅の値と確認の道具を持つことを照合する。
- 失敗した全テスト同時のイベント列に `check_test_width.py --at-most 16` をかけると「354 tests ran at once, above 16」で失敗し、幅 16 の列は合格した。
- `test_tsan_runtime.py` の製品の検査の写しに、幅の値のファイルを加えた。

## 影響と残り

- `check-swift` の所要は増えない（上の表）。テストをまたぐ同時の状態は、通常の実行では 16 本までしか重ならない。競合の検出は従来どおり TSan の全テスト同時の実行が担う。
- 5 秒を超えるシステム全体の停止は、ADR 0210 と同じくこの対策でも吸収できない。
- 復旧の契約が全テスト同時で 20 秒かかる原因（どのテストが主アクターを長く使うか）は特定していない。
