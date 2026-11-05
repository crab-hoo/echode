# ADR 0233: 見送りの理由を検査ごとにし、検査の罠を減らす

- 日付: 2026-11-03
- 状態: 採用
- 関連: [ADR 0212](0212-verification-tiers.md)、[横断監査](../reviews/2026-11-02-cross-cutting-audit.md)の XCA-08・09

## 背景

開始時の HEAD は `2ecc601`。

- XCA-08: `scripts/verify.py` の `--defer` は複数指定でき、`--reason` は 1 つだけだった。
  10 件が同じ理由を持ち、`spike-isolation` や `spike-audio` の理由がその検査の目的を説明していなかった。
  入力が変わらない間は見送りが有効なので、前の作業の理由（`advisories`）も残り、気付く場所がなかった。
- XCA-09: 次の 3 つが、避けられる高コストの罠だった。
  - `--defer` は記録だけに見えるが、選択された自動検査をすべて実行する。
  - `make -s verify` は `MAKEFLAGS` の違いで全検査の結果を無効にする。2026-11-01 に 36 分の結果を失った。
  - 空き容量は記録するだけで、事前確認がない。2026-10-31 の新しい worktree の初回で、空きは約 11 GB から約 1.2 GB へ減った。

## 決定

- 見送りは `--defer ID=理由` で検査ごとに理由を持つ。
  `--defer ID --reason 理由` は 1 件の時だけ受け付け、`--reason` を 0 件や複数の検査に付けると拒否する。
  同じ検査の二重指定と空の理由も拒否する。
- 見送りの記録に `deferred_at` を持たせる。`make verify` の最後と `report.md` に、選択された見送りを検査名・理由・記録日時で示す。
  前の作業の理由は失効させない（入力が同じなら見送りの判断も同じである）。表示で気付けるようにし、必要なら記録し直す。
- 記録だけの入口として `--record-only` を足す。事前確認・ビルド・検査を実行せず、実行時間の履歴にも書かない。
  `--run`・`--only`・`--force`・`--plan` とは併用できない。完了の判定には続けて `make verify` が要る。
  終了時のフックの案内もこの入口を示す。
- ツールの識別では、表示だけを変える `make` のフラグを除く。対象は `-s`・`-w` と、`--silent`・`--quiet`・`--print-directory`・`--no-print-directory`。
  `-i`・`-k`・`-n` などの結果を変えるフラグは残す。
  make 3.81 は 1 文字のフラグを先頭の 1 語にまとめる（`sk`）ので、その語から `s`・`w` だけを除く。
  回帰はこのマシンの `make` を実際に起動して確かめる。
- ビルドする自動検査に空き容量の事前確認を足す（`scripts/check_disk_space.py`）。
  - `disk-space`: 3 GiB。`target/debug` がなければ 6 GiB を足す。
  - `disk-space-mutations`（`check-product-contracts`・`check-contract-controls`）: 上に加え、反例の `cargo` がなければ 4 GiB を足す。
  - 3 GiB は、既存のキャッシュでの 1 回の最大の減少（2026-11-02 の 1.4 GiB）の約 2 倍である。
    冷えたキャッシュの値は、2026-10-31 の初回の約 10 GB を 2 つへ分けた見積もりで、実測ではない。止めすぎや足りなさが分かったら直す。
  - `check-format`・`check-verification`・`check-tsan-runtime` はほぼ書かないので対象外にする。手動の計測は明示の実行なので対象外にする。

## 残る範囲

- Swift のテストの `.timeLimit`（XCA-09）: Swift Testing の 106 ファイルのうち `@Suite` は 6 個で、まとめて付ける場所がない。
  1 件の待ち続けは、従来どおり検査全体の期限と [デバッグ手順](../debugging.md) の扱いで止める。
- 監査が求めた「検査の目的と今回の影響の確認」そのものは、理由の文面を機械で判定できないので強制しない。

## 回帰と基盤

- `test_verification_execution.py` に次を足した。
  - `test_each_deferral_keeps_its_own_reason_and_a_shared_reason_is_refused`
  - `test_every_selected_deferral_and_its_reason_reach_the_final_output`
  - `test_record_only_writes_deferrals_without_preflights_checks_or_timing`
  - `test_display_only_make_flags_keep_the_identity_but_outcome_flags_change_it`
  - `test_build_checks_require_free_space_sized_for_their_cold_caches`（反例の `cargo` の場所を `check_product_contracts.MUTATIONS` と照合する）
- `measure_verification_failures.py` の引数に、ADR 0212 で増えた `full` が欠けていたので足した。
- `scripts/verify.py` の変更は検査の識別値に入るので、全自動検査が選ばれる。
