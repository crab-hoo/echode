# ADR 0119: AI エージェント作業時のトークン効率と文書・テスト・検証出力の整理

- 日付: 2026-10-19
- 状態: 承認

## 背景

Echode の開発では AI エージェントがコードとドキュメントの読み書き、テストの実行、`make verify` による検査を一貫して行うが、以下の 4 つの構造的要因により 1 セッションあたりのトークン消費が著しく膨張していた。

1. **テスト・ビルド標準出力の全件出力**:
   - `Makefile` の `check-base`、`check-verification`、`rtsan`、`spike-*` や `scripts/check-shell.sh`、`scripts/check_product_contracts.py` が静音オプションなしで実行され、通過した数百件の `test ... ok` / `✔ Test ... passed` がそのまま出力されていた（`check.log` 単体で約 76 KB / 1,503 行、`check-verification.log` で約 15 KB / 104 行）。
2. **`make verify` 失敗時の巨大ログ読込**:
   - `scripts/verify.py` の `run_check` は失敗時に `{name}: failed ({name}.log)` の 1 行しか出さず、エージェントが末尾の失敗 1 件を特定するためだけに巨大な `target/verification/*.log` を開く必要があった。
3. **`make verify-plan` と `target/verification/report.md` の変更ファイル重複列挙**:
   - 選択された各検査項目（最大 19 項目）ごとに変更されたファイル一覧が毎回フルパスで列挙され、変更ファイルが 15〜40 個あると `report.md` と `make verify-plan` の出力が約 22 KB に膨らんでいた。
4. **ドキュメントと製品ソースコードの構造的肥大化**:
   - **① 1 行 1,000〜2,400 バイト超の巨大行**: ADR 追記のたびに既存段落の末尾へ改行なしで追記し続けた結果、行範囲指定のファイル閲覧ツールで 100 行読むだけで 20〜46 KB を消費していた。
   - **② 単一ファイルへの全ドメイン同居**: `docs/ARCHITECTURE.md`（112.7 KB）に §4〜§17 の全ドメインが同居していた。
   - **③ 6 つの正本ファイルへの同じ ADR 要約の重複追記**: `ARCHITECTURE.md`、`PLAN.md`、`interaction-design.md`、`verification.md`、`development.md`、`README.md` に同じ実装範囲・契約説明が重複記述されていた。
   - **④ 製品 Rust ファイル後半の巨大なインライン `#[cfg(test)] mod tests`**: `stream.rs`（66.7 KB のうち後半 862 行がテスト）や `playback.rs`（89.2 KB のうち後半 500 行以上がテスト）など、27 個の製品ソースファイルの末尾に 100〜1,150 行の単体テストが同居していた。

## 決定

1. **ビルド・テスト出力の静音化**:
   - `Makefile`、`scripts/check-shell.sh`、`scripts/spike-bridge.sh` の `cargo build`、`cargo clippy`、`cargo test`、`swift build`、`swift test` に `-q` を付与し、`cargo deny` に `-L error` を付与する。
   - `check-verification` の `-v` と、`rtsan` および `spike-*` の `--nocapture` を外す。
   - `scripts/check-shell.sh` の Swift テスト実行と `scripts/check_product_contracts.py` は、全件通過時は 1 行の要約のみを出力し、失敗時のみ詳細を標準エラーへ出す。
2. **`scripts/verify.py` の失敗抜粋表示と選択根拠の要約**:
   - 検査失敗時は `failure_excerpt(log_path)` により、ログ内の失敗箇所（`FAILED`、`error[`、`panicked at` 等）と末尾抜粋を最大 40 行に絞って標準エラーへ即時表示する。
   - `make verify-plan` と `target/verification/report.md` の「選択の根拠」は `summarize_reasons(reasons, limit=3)` で先頭 3 件＋残件数（`ほか N 件`）に要約する。
3. **ドキュメントの領域分割・重複排除・行折り返し**:
   - `docs/ARCHITECTURE.md`（112.7 KB → 約 14 KB）は全体構成（§1〜§3）と全節の見出しアンカー・要約索引とし、§4〜§17 の詳細設計は `docs/architecture/` 配下の 5 ファイル（`model-and-history.md`、`audio-engine.md`、`plugins-and-devices.md`、`ui-and-metal.md`、`api-qa-and-distribution.md`）へ分離する。
   - `docs/PLAN.md`、`docs/interaction-design.md`、`docs/verification.md`、`docs/development.md`、`docs/debugging.md`、`docs/status.md`、`docs/phase2.md`、`docs/README.md`、`AGENTS.md` の重複記述を正本へのリンクへ統合し、1 行あたり 600 バイト以下に折り返す。
   - `scripts/tests/test_diagnostics.py` で主要ドキュメントのリンク切れと 1 行の最大バイト長（600 バイト以下）を自動検査する。
4. **製品 Rust ファイルからの大規模単体テスト分離**:
   - 100 行以上のインライン `#[cfg(test)] mod tests` を持つ 27 個の製品ソースファイルについて、テスト本体を同ディレクトリの `<stem>_tests.rs`（`echode-ffi` は `tests.rs` と `snapshot_ownership.rs`）へ切り出し、製品ファイル末尾には `#[cfg(test)] #[path = "<stem>_tests.rs"] mod tests;` のみを残す。

## 結果

- 製品 Rust ファイル 27 個の合計行数が **23,933 行 → 12,869 行（−11,064 行 / −46%）** に削減され、実装ファイル閲覧時にテストフィクスチャがコンテキストへ混入しなくなった。
- 主要ドキュメント 8 ファイル（`README.md`、`development.md`、`verification.md`、`status.md`、`debugging.md`、`phase2.md`、`PLAN.md`、`interaction-design.md`）と `ARCHITECTURE.md` の合計サイズが **356 KB → 105 KB（ドメイン分割後の `docs/architecture/*.md` 5 ファイル計 35 KB を含めても 140 KB、−61%）** に削減され、全行が短く折り返された。
- `make check-verification`、`make verify-plan`、`make check` の標準出力・ログ出力が数行〜十数行に収まり、失敗時はログを開かずに原因箇所を特定できるようになった。
