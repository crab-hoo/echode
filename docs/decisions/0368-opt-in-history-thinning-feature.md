# ADR 0368: 履歴の間引きとバンドルを非既定の feature へ閉じる

- 日付: 2026-11-15
- 状態: 採用
- 関連: [DIA-11](../reviews/2026-11-13-detailed-implementation-audit.md#dia-11-接続していない間引き)、[ADR 0046](0046-history-thin.md)、[ADR 0211](0211-opt-in-history-spike.md)、[ADR 0230](0230-save-durability.md)、[ADR 0300](0300-typed-thinning-errors.md)、[ADR 0361](0361-unshipped-configurations-only-on-github.md)

## 問題と根拠

`crates/echode-history/src/thin.rs`・`error_thin.rs` および関連の `ThinningError`・`pack_and_thin` は製品から呼ばれていないにもかかわらず既定のビルドに含まれ、`flate2` と `gix-pack/streaming-input` を製品グラフへ引き込んでいた。
現行の間引きはコミット書き直し後に `.git/echode-commit-chain`（[ADR 0230](0230-save-durability.md)）と不整合になり得て、自動保存が並行して書いたばらのオブジェクトも排他なしに削除する（DIA-11）。

## 比較と決定

1. 既定ビルドに残したまま注記だけ足す: 製品ビルドから未接続コードを呼べる状態と、`flate2`・`gix-pack/streaming-input` の常時ビルドが残る。
2. 既存の Phase 0 検証 `spike` feature へ混ぜる: `spike = []` を前提にする既存回帰（`test_history_spike.py`）や Phase 0 履歴モデル検証の境界と混ざる。
3. 非既定の `thinning` feature を新設し、`thin`・`error_thin`・`ThinningError`・間引き専用の `GitOperation` 7 変種・統合テスト `tests/thin.rs`・`tests/thin_errors.rs` をそこへ閉じる。`flate2` と `gix-pack` は `optional = true` にして `thinning = ["dep:flate2", "dep:gix-pack"]` からだけ有効にする。

3 を採る。通常の `make verify` と製品ビルドでは `thin` 系と `flate2` が外れ、GitHub `full` や節目のチェックで動かす `Makefile` の `check-spikes`・`check-portable` が `--features thinning` の clippy とテスト（`--lib --test thin --test thin_errors`）を継続検査する（[ADR 0361](0361-unshipped-configurations-only-on-github.md)）。
将来製品へ間引きを接続する時は、先に自動保存・`echode-commit-chain` との排他と大規模パック対策を設計した上で既定へ戻す（`2026-11-08-open-items-register.md` F13）。

## 回帰と基盤

- `scripts/tests/test_history_thinning.py`: `default` feature に `thinning` が入らないこと、`flate2` と `gix-pack` が optional であること、`tests/thin.rs`・`tests/thin_errors.rs` が `thinning` 必須であること、`Makefile` の `check-spikes`・`check-portable` が `--features thinning` を検査することを検証する。
- `check_typed_errors.py`・`test_history_spike.py` は変更せず従来どおり通る。
- 設計書（`model-and-history.md`）、開発手順（`development.md`）、デバッグ手順（`saving-history-and-api.md`）、監査文書・未完了台帳を更新する。実機確認は追加しない。
