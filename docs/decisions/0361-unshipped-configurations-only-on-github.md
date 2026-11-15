# 0361. 出荷しない構成は GitHub だけでビルドする

- 日付: 2026-11-15
- 状態: 採用
- 関連: [ADR 0211](0211-opt-in-history-spike.md)、[ADR 0213](0213-opt-in-engine-spike.md)、[ADR 0269](0269-github-public-ci-scope.md)、
  [ADR 0276](0276-priority-order-and-delegated-decisions.md)（優先順位）、[ADR 0327](0327-instrumented-builds-only-on-github.md)、
  [ADR 0328](0328-one-cargo-selection-for-the-task-tier.md)
- 置き換え: ADR 0328 の「`check-base` は橋渡しスパイクの clippy とテストを `-p echode-bridge-spike` で別に行う」と、
  「feature を名指す契約は、`check-base` の `spike` の実行と同じ選択を使う」

## 背景

ユーザーから「ローカルの負荷が増えてきたので、できるものは検証を GitHub 側に移してプッシュする」よう依頼された。

2026-11-12 12 時（ADR 0327 の後）から 11-15 3 時（UTC）までの `timing-history.jsonl`（`make verify` 117 回）で、手元の時間は次のとおりだった。

| 検査 | 実行回数 | 中央値 | 合計 |
| --- | --- | --- | --- |
| `check` | 49 | 442 秒 | 5.9 時間 |
| `check-product-contracts` | 42 | 59 秒 | 0.7 時間 |
| `check-swift` | 44 | 48 秒 | 0.6 時間 |
| `check-verification` | 72 | 28 秒 | 0.6 時間 |

`check` は 11-13 の 316〜348 秒から、11-14 以降は 435〜880 秒に延びていた。
最後の `check`（11-15 2:57、492 秒）のログでは、テストの実行が次のとおりだった。

- 既定のメンバーの `cargo test`: 143 バイナリで 409 秒。
- `cargo test -p echode-history --features spike --lib --test roundtrip`: 55 秒。
  うち `--lib` の 52.6 秒は、直前の既定の実行（64 件、53.2 秒）と同じ単体テストを `spike` 付きでもう一度動かしたものだった。

ADR 0327 は、手元の作業の段階を「製品と同じ 1 つの構成のビルドとテスト」に限ると決めた。
それでも次の構成が作業の段階に残っていた。どれもアプリが出荷しない構成で、Cargo は `-p` と feature の組ごとに crate の別の写しを作る（ADR 0328）。

- `check-base`: 履歴の `spike`（clippy・テスト）、エンジンの `spike`（計測のバイナリ・clippy・単体テスト）、橋渡しスパイク（clippy・テスト）。
- `check-swift` の `make check-bridge`（`scripts/spike-bridge.sh`）: 橋渡しスパイクの release を `bindgen,fault-injection` と、別の出力先の `async-callback` 付きで 2 回ビルドし、Swift の検証プローブを動かす。
- `check-bridge-features`（橋渡しの領域の変更）: 橋渡しスパイクの全 feature。
- 作業の段階の契約 `engine-measurement-completion`: エンジンの `spike` のテストのビルド。

橋渡しスパイクは `echode-model` に依存する。`echode-model` を 1 行変えると、既定の 2 つの写しに加えて、上の 4〜6 通りの写しを作り直していた。
同じ日の空きは 12 GiB（使用率 98%）、`target` は 40 GB（`debug` 36 GB、`release` 1.6 GB、`bridge` 471 MB）だった。

## 決定

### 出荷しない構成を全体の段階へ移す

- 新しい全体の段階の検査 `check-spikes` が、`check-base` から外した 7 つの Cargo の呼出しと `make check-bridge` を実行する。
- `check-bridge-features` に `tier: full` を付けた。
- GitHub の `full` に `spikes` ジョブを足し、`cargo fetch --locked` の後に `make check-spikes` と `make check-bridge-features` を実行する。
  後者は前者の失敗でも動かし、片方の失敗がもう片方を隠さないようにした。
- `check-swift` は `make check-bridge` を呼ばない。入力から `spikes/bridge/**`・`scripts/spike-bridge.sh`・`scripts/run-bridge-validation.py` を、
  パッケージから `echode-bridge-spike` を外した。
- `check` の入力の `spikes/**` は残した。`check_typed_errors.py`・`check_workspace_lints.py`・`cargo deny` が、ワークスペースの一員の橋渡しスパイクも読むためである。

### 作業の段階の契約は出荷する構成だけを証明する

- `check_product_contracts.py --tests-only` は、出荷しない feature を名指す契約（`target` に `--features` を持つ Rust の契約）を実行しない。
  全体の段階が、その契約の通常テストを契約ごとの `cargo test -p` で、反例とともに実行する。
- 作業の段階の Rust の契約は、既定のメンバーの 1 回の `cargo test --no-run` だけから探す。
  `rust_selection` を `rust_target` にし、feature を名指す契約が渡れば理由付きで拒否する。ログは `product-contracts/rust-test-build.log` の 1 つになった。

### 照合

- `scripts/tests/test_cargo_selection.py` は、作業の段階の Cargo の呼出し（別のプロファイル・出力先・ツールチェーンを除く）に
  `-p`・`--package`・`--features`・`--all-features`・`--workspace`・`--all` を認めない。
  以前は「feature を足すか、既定のメンバーの外を選ぶ `-p`」を認めていた。

## 比べた案

- 作業の段階に残し、履歴の `spike` のテストを `spike` でしか動かない分に絞る。
  重複した 53 秒は消えるが、4〜6 通りの写しのビルドと clippy は残る。ADR 0327 の原則とも合わないので採らない。
- `check-product-contracts` も GitHub へ移す（ADR 0327 が別の作業に回した案）。
  中央値 59 秒が減るが、必須テストの削除・無視・0 件を止める手元の門がなくなる。コミットの前に止めるほうが堅牢なので、今回も採らない。
- `check`・`check-swift`、または長いテストのバイナリを GitHub へ移す。
  最も長いのは `echode-plugin-host` の `audit`（131.9 秒。PLG9 が FSEvents の監視の開始と破棄を 100 回繰り返す）と `echode-cli` の `render`（68.5 秒）で、どちらも製品の回帰である。
  `main` に未検査のコミットが積まれるので採らない（ADR 0327 と同じ理由）。
- 手元の M4 をセルフホストランナーにする。ADR 0269 のとおり、公開リポジトリでは採らない。

## 結果と制約

- `spike` の feature の中だけのコンパイルエラー・lint・テストの失敗と、橋渡しスパイクの ABI 検証の失敗は、プッシュの後の `full` でしか分からない。
  `ContinuationHandoff.swift` を変えた時の、橋渡しの検証プローブ（`BRIDGE_VALIDATION` と故障の注入）も同じである。製品の Swift のテストは手元に残る。
- `engine-measurement-completion` は、手元の作業の段階では証明されない。
- 手元の `make verify-full` は、この 2 つの検査も含む。
- 作業の段階が使わなくなった `target/bridge`（471 MB）は消した。手元の `make verify-full` と `make spike-bridge` が作り直す。
  `target/debug` の中の `spike` の写しは、パッケージ単位でしか消せないので残した。`target/release` は手動の計測も使うので残した。

### 変更後の最初の実行

- 2026-11-15 12:49〜13:10（JST）の `make verify` は合格した。`check` 896 秒、`check-product-contracts` 245 秒、`check-swift` 36 秒だった。
- この実行は比較に使えない。開始時の負荷平均は 7.8 で、macOS の `ApplicationsStorageExtension`（CPU 98%）・`StorageManagementService`（74%）・`dasd`（97%）が 66 分続いていた。
  Rust の再コンパイルは起きておらず（`target/debug/deps` に実行中の更新なし）、テストの実行だけで 500 秒（前回 409 秒）だった。
- 外した分は、テストでは履歴の `spike` の 55 秒とエンジンの `spike` の 2 秒、ビルドでは Rust の編集ごとの 4〜6 通りの写しである。
  同じ負荷での前後の比較は、ADR 0210 の手順（交互に流す）で次に測る時に行う。

### GitHub の初回の実行

- `1bf4a63` のプッシュ（未プッシュだった前の 14 コミットを含む）で、`full`（実行 43675740924）の 10 ジョブと `check`（実行 43675740936）が合格した。
- 新しい `spikes` ジョブは 8.3 分だった。`spike-bridge.sh` の検証プローブが GitHub のランナーで動いたのは初めてである。
- `full` 全体は、最後に始まった ASan のジョブの終了まで 45 分だった。

## 回帰・検査基盤

- `test_task_checks_build_the_default_members_once` が、作業の段階の Cargo の呼出しに `-p` と feature の指定がないことを検査する。
- `test_a_task_builds_only_the_shipped_features_and_leaves_the_others_to_the_full_tier` が、
  `rust_target` の拒否、`cargo test --no-run` に選択がないこと、作業の段階が feature の契約を実行せず全体の段階が実行すること、
  feature を名指す契約と同じ `-p` と feature の組が `check-spikes` にあることを検査する。
- `test_history_spike.py`・`test_engine_spike.py` の `test_default_and_spike_checks_remain_separate` が、`check-base` に `--features` がなく、`check-spikes` に `spike` の実行があることを検査する。
- `test_the_full_tier_is_exactly_the_slow_evidence` が全体の段階の 9 件を、既存の `test_full_ci_runs_every_full_tier_check_with_complete_shards` が `full.yml` との一致を検査する。
