# 0211. Phase 0 の履歴モデルを非既定の feature へ分離する

- 日付: 2026-11-01
- 状態: 採用
- 対象: [構造監査](../reviews/2026-10-30-structure-audit.md) SPG-10 の履歴側
- 関連: [S4](../spikes/S4-history.md)、[モデル・履歴設計](../architecture/model-and-history.md)

## 背景

開始時の HEAD は `ae9e7db`。
`echode-history` の `project.rs`・`format.rs`・`store.rs` は、Phase 0 の測定用の別モデルと Git 保存を持つ。
製品は `echode_model::Project` と `Autosave`・`DocumentHost` を使い、この別モデルの呼出しは `tests/roundtrip.rs` だけだった。
別の時間単位・JSON 形式・履歴が、製品の通常ビルドでもコンパイルされていた。
誤ったモデルを製品が実際に使った不具合は再現していない。

## 決定

- `echode-history` に空の `spike` feature を追加し、既定では有効にしない。
- `format`・`project`・`store` と公開の `spike` モジュールを、この feature がある時だけコンパイルする。
- `roundtrip` の Cargo ターゲットにも `required-features = ["spike"]` を付ける。
  ファイル内の製品 `Autosave` の計測も、従来どおり `make spike-history` で実行する。
- `make check` は既定の workspace の lint・全テストを先に行い、別の Cargo 呼出しで履歴 spike の lint・単体テスト・非計測の往復回帰を行う。
  workspace 全体へ feature を有効にして製品の分離を隠さず、Phase 0 の通常回帰も失わない。
- `make spike-history` は feature を明示する。計測の入力・期待値・報告の形式・出力先は変えない。

新しい crate や依存は追加しない。製品の保存形式・編集・復旧・音声処理は変更しない。
エンジンの `spike` は共有する機器問い合わせとの境界整理が別途必要なので、この決定の対象外とする。
SPG-10 全体を完了とは扱わない。

## 回帰と基盤の保守

- `scripts/tests/test_history_spike.py` は非既定 feature、全対象モジュールの条件、テストターゲットの条件を確認する。
  最初の非既定 feature のアサーションは、変更前に失敗させた。製品の誤動作の再現とは区別する。
- 同じテストで実際の Cargo の既定 workspace の依存解決を読み、推移的な feature 統合でも履歴 spike が有効になっていないことを確認する。
- 既定と feature 付きの検査、および計測入口の明示指定を検査する。検査は既存の `check`・`check-verification` に含める。
- 実行方法は [開発手順](../development.md)、検査の選択は `scripts/verification.json` が引き続き正本である。
- ログ形式・外部入力・失敗の種類は増えない。`doctor` と `debug-report` の収集範囲の変更は不要である。
  feature の指定漏れでの Cargo ターゲット拒否だけ、デバッグ手順へ追記する。

S4 の過去の計測値は保持し、今回の版での性能測定の合格へ移し替えない。
