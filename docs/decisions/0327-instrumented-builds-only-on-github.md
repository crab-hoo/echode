# 0327. 計装したビルドと反例は GitHub だけで実行する

- 日付: 2026-11-12
- 状態: 採用
- 関連: [ADR 0212](0212-verification-tiers.md)、[ADR 0269](0269-github-public-ci-scope.md)、[ADR 0271](0271-task-tier-duplicate-runs.md)、
  [ADR 0276](0276-priority-order-and-delegated-decisions.md)（優先順位）
- 置き換え: ADR 0212 の「作業の段階でサニタイザを選ぶ条件」「作業の段階で反例を選ぶ条件」「合格済みの変更で選び直さない」、
  [ADR 0232](0232-verification-escapes-and-expected-red.md) の ENV-01 の修正（削除行と C の変更行を数える選択）、
  ADR 0271 の「RTSan と変えた契約の反例は作業の段階に残す」

## 背景

ユーザーから「ローカルの負担を減らすため、ストレージやマシンリソースを大きく消費する検査はできるだけ GitHub 側で行う」と指定された。
プッシュして `full` の結果を確かめ、問題があれば直すことも、同じ依頼に含まれていた。

2026-11-12 の手元の Mac は、空きが 11 GiB（使用率 98%）だった。
同じ日の早い時刻に、`make verify` が空きを使い切っている（[空き容量の不足](../debugging/verification-and-build.md)）。
`target/` の内訳は `debug` 38 GB、`bridge`（ASan・TSan・Miri）5.1 GB、`aarch64-apple-darwin`（fuzz）1.3 GB、`toolchains`（TSan のランタイム）246 MB だった。
反例の `product-contract-mutations`（8.9 GB）は、同じ日の空き不足で消した後だった。

2026-11-05 の ADR 0269 の追記から 2026-11-12 までの `timing-history.jsonl`（`make verify` 296 回）で、作業の段階が手元で動かした重い検査は次のとおりだった。

| 検査 | 実行回数 | 中央値 | 合計 | 選ばれた理由 |
| --- | --- | --- | --- | --- |
| `check-product-contracts` | 81 | 180 秒 | 6.0 時間 | 毎回。置換箇所が変わった契約の反例を含む |
| `rtsan` | 70 | 73 秒 | 2.0 時間 | 音声の領域の変更 |
| `check-bridge-asan` | 19 | 194 秒 | 1.5 時間 | `unsafe` の変更、自身のスクリプト |
| `check-contract-controls` | 3 | 1,799 秒 | 1.4 時間 | 自身のスクリプト |
| `check-bridge-tsan` | 23 | 141 秒 | 1.3 時間 | `unsafe` の変更、自身のスクリプト |
| `check-bridge-miri` | 21 | 39 秒 | 0.3 時間 | `unsafe` の変更、C ABI のファイル |
| `check-fuzz` | 2 | 408 秒 | 0.2 時間 | 自身のスクリプト |

これらはどれも、製品と別のビルドを持つ。
サニタイザ・Miri・fuzz は固定 nightly と計装した標準ライブラリ、RTSan は別の feature と取得したランタイム、反例はソースのコピーと別の Cargo の出力先である。
GitHub の `full` は、同じ検査を `main` へのプッシュごとに全部実行している（ADR 0269）。

## 決定

### 作業の段階は全体の段階を選ばない

- `make verify`（作業の段階）は、`tier: full` の検査を選ばない。`unsafe` の変更でも、その検査自身のスクリプトの変更でも同じである。
- `task_inputs`・`task_unsafe`・`task_requires` と、`unsafe` の脱出口を差分から探す処理、合格した変更を覚える `covered` を消した。
  `load_policy` は、この 3 つの鍵を持つ検査を拒否する。効かない鍵が登録簿に残らないためである。
- 全体の段階の検査は `make verify-full` と GitHub の `full` だけが実行する。

### RTSan を全体の段階へ移す

- `rtsan` に `tier: full` を付け、`full.yml` に 1 つのジョブとして足した。
  `make rtsan` の最初の `make bundled-plugins` はオフラインでビルドするので、その前に `cargo fetch --locked` を置く。
- 手元の `target/debug/echode-plugin-host` を RTSan 版で上書きすることも、作業の段階ではなくなる。

### 作業の段階の製品契約は通常のテストだけにする

- `check-product-contracts` は `--tests-only` で動き、全必須契約のテストを ADR 0271 のまとめた実行で通す。
  全反例が現在のソースに適用できることは、ビルドせずに引き続き確かめる。
- 反例の実行は `check-contract-controls`（全体の段階）だけにした。
  置換箇所・前後 20 行・テスト本体の差分で反例を選ぶ `control_sites` は消した。
- 作業の段階は、手元の全体の段階が残した現在の入力の反例の合格を、記録から消さない。
- 事前確認は `disk-space-mutations` から `disk-space` に変えた。反例のコピーを作らないためである。

### 手元とプッシュの運用

- 手元の作業の段階は、製品と同じ 1 つの構成のビルドとテストに限る。コミットの前の検査はこれで行う。
- プッシュは引き続きユーザーが決める（AGENTS.md）。依頼されたプッシュの後、エージェントは対象のコミットの `full` を確かめ、失敗を同じ作業で直す。
- `make verify` の出力と `report.md` は、毎回、全体の段階の 7 件を「全体検査で未確認」と示す。合格とは扱わない。
- `make doctor` と `make agent-context` は、TSan のランタイムの未準備を、手元の全体の段階だけの準備として示す。
  作業の段階では `make setup-bridge` を要らないので、エージェントがランタイムを作り直さないためである。

## 比べた案

- 選択は残し、実行だけを GitHub へ回す（「委ねた」状態を記録する）。
  どの検査が今回の変更に関わるかは分かるが、`full` はプッシュごとに全件を実行するので、その情報で行動は変わらない。
  選択・記録・表示の経路が増え、ADR 0276 の「入り組んだ構造は堅牢性の損失」に当たるので採らない。
- `check`・`check-swift` も GitHub へ移す。
  手元にコミット前の検査がなくなり、`main` のコミットがテストなしで積まれる。堅牢性を下げるので採らない。
- 契約の通常テストも GitHub へ移す。
  契約は `cargo test -p <package>` で動き、`-p` ごとに依存の feature の統一が変わって、依存の木を別に作る。
  2026-11-12 に `cargo test --no-run --message-format=json` で、`echode_engine` の rlib のハッシュが `--workspace`・`-p echode-engine`・`-p echode-plugin-host` の 3 通りで全部違うことを確かめた。
  `target/debug/incremental` には、同じ日に `echode_plugin_host` が 71 通り、`echode_engine` が 70 通りあった。
  ただし、削除・無視・0 件の必須テストを拒否する手元の門がなくなる。どの feature の構成を契約の証拠にするか（ADR 0148）も決め直しが要る。
  この作業では変えず、別の作業で扱う。
- `resolver.feature-unification = "workspace"` で `-p` の違いを消す。
  cargo 1.96.1 は `warning: ignoring resolver.feature-unification without -Zfeature-unification` で無視した。安定版では使えない。
- 自分の M4 をセルフホストランナーにする。ADR 0269 のとおり、公開リポジトリでは採らない。

## 結果と制約

- `unsafe` の変更、RTSan が見る音声スレッドの禁止操作、契約のテストの検出力は、プッシュの後にしか確かめられない。
  エージェントは頼まれない限りプッシュしないので、未確認のコミットが手元に積まれることがある。
  `full` の失敗は公開の `main` に赤として残る。
- 2026-11-02 の監査の ENV-01 の再現（`unsafe` の削除と C の変更でサニタイザが選ばれない）は、対象の選択ごとなくなった。
  `full` は変更の内容に関係なく全件を実行するので、取りこぼしは起きない。再現のテストは消し、監査の文書に経緯を足した。
- 手元の `make verify-full` は残る。CI で再現しない失敗を調べる時と、依頼された時に使う。
- 手元の全体の段階だけが使うビルド先は、作業の段階では再利用されないので消した。
  `target/bridge/thread`（3.5 GB）・`miri`（1.1 GB）、`target/aarch64-apple-darwin`（1.3 GB）、`target/toolchains/tsan`（246 MB）、`target/fuzz/corpus`（45 MB）で、空きは 11 GiB から 17 GiB になった。
  `target/bridge` のほかの出力（`async-callback` など）は `check-swift` の `spike-bridge.sh` が使うので残した。

### 既存の `full` の失敗（fuzz の準備）

- この作業の前、2026-11-09 と 11-11 の `full` は、fuzz のジョブだけが `make setup-fuzz` で失敗していた（実行 42599052380・42933587976）。
  `cargo install cargo-fuzz` が `serde_derive` を `mis-aligned LINKEDIT string pool` で読み込めなかった。ADR 0113 の macOS 27 の strip の問題である。
- `cargo install` はリポジトリの `.cargo/config.toml` を読まないので、`strip = "none"` が届かない。
  2026-11-12 に `cargo install -v` で、既定では `serde_derive` に `-C strip=debuginfo` が付き、`CARGO_PROFILE_RELEASE_STRIP=none` では付かないことを確かめた。
  手元の macOS 27.0・Xcode 27.1 では、既定のままでも導入できた。ランナー（`xcode-27-arm64` 20261104.0337）でだけ落ちる。
- `scripts/fuzz.sh`・`scripts/coverage.sh`・`make setup` の `cargo install` に `CARGO_PROFILE_RELEASE_STRIP=none` を付けた。

### 初回の実行

- `a82fca1` のプッシュ（未プッシュだった前の 15 コミットを含む）で、`full`（実行 43197071599）の 9 ジョブが全部合格した。プッシュから約 42 分だった。
- `rtsan` は 3.1 分で、3 つの crate の `under_rtsan` のテスト 85 件が M1 のランナーで合格した。
- fuzz は 10.8 分で、5 つの対象を各 61 秒動かした。ADR 0290 で足してから初めての合格である。
- ASan 20.0 分、TSan 18.2 分、Miri 4.4 分、反例の 4 シャードは 20.8〜35.7 分だった。`check`（実行 43197071484）も合格した。

## 回帰・検査基盤

- `test_no_change_selects_the_full_tier_in_a_task` が、検査自身のスクリプト・C ABI・音声・`unsafe` を含むファイルの変更でも、作業の段階が全体の段階を選ばないことを検査する。
- `test_tiers_are_validated` が、`task_inputs`・`task_unsafe`・`task_requires` を持つ検査の拒否を検査する。
- `test_the_full_workflow_is_the_routine_run_of_the_full_tier` と既存の `test_full_ci_runs_every_full_tier_check_with_complete_shards` が、`full.yml` の `rtsan` を含む全件と、その前の `cargo fetch` を検査する。
- `test_a_task_runs_every_test_and_leaves_every_counterexample_to_the_full_tier` と
  `test_a_task_runs_no_counterexample_and_the_full_tier_does_not_reuse_its_batch_evidence` が、作業の段階で反例を動かさないこと、
  全体の段階が作業のまとめた実行を再利用しないこと、現在の反例の合格を記録に残すことを検査する。
- `test_tool_installs_keep_release_symbols_for_macos_27` が、Makefile と `scripts/*.sh` の全 `cargo install` に `CARGO_PROFILE_RELEASE_STRIP=none` があることを検査する。
  修正前の 3 行で失敗することを確かめた。
- `test_diagnostics` と `test_agent_context` が、`setup-bridge` の案内に全体の段階だけの準備であることを添えると検査する。
- `test_an_edit_to_a_full_tier_input_leaves_it_to_the_full_tier` が、全体の段階の入力の変更で作業の段階が合格を失効させ、選ばないことを検査する。
