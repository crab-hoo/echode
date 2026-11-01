# 0213. Phase 0 のエンジン計測を非既定の feature へ分離する

- 日付: 2026-11-02
- 状態: 採用
- 対象: [構造監査](../reviews/2026-10-30-structure-audit.md) SPG-10 のエンジン側
- 関連: [ADR 0211](0211-opt-in-history-spike.md)、[S2](../spikes/S2-engine.md)、[エンジン設計](../architecture/audio-engine.md)

## 背景

開始時の HEAD は `b9c3177`。
履歴側の Phase 0 モデルは分離済みだが、エンジンの固定 1,000 ノードのグラフ、待機処理、計測器と専用バイナリは既定ビルドに残っていた。
製品の再生は `playback` を使い、このグラフを使わない。
一方で `device.rs` は計測に加えて、製品も使う `OutputInventory`・`IoWorkgroup` と公開 HAL の問い合わせを持っていた。
誤ったグラフを製品が実際に使った不具合は再現していない。

## 決定

- `echode-engine` に空の `spike` feature を追加し、既定では有効にしない。
- `graph`・`wait` と公開の `spike` モジュールを、この feature がある時だけコンパイルする。
- 計測の制御ループ・レポート・IOProc・設定の復元・停止通知を、`device_measurement.rs` の子モジュールへ移す。
  `device.rs` の製品用の問い合わせはそのまま残し、計測だけが使う型・設定書込み・既存の外部関数宣言にも条件を付ける。
  OS API の手書き宣言を新しいファイルへ増やさない。
- `echode-engine-spike` の Cargo ターゲットに `required-features = ["spike"]` を付ける。
- `make check` は既定の workspace を検査した後、別の Cargo 呼出しでエンジン spike のバイナリのビルド・lint・単体テストを行う。
  製品の分離を workspace 全体の feature 統合で隠さず、非計測のスケジューラ回帰も維持する。
- `make spike-engine`・`make spike-engine-smoke` は feature を明示する。
  必須契約 `engine-measurement-completion` も feature と移動後のテスト・反例の所在を明示する。

製品の再生・書出し・機器問い合わせ、計測の入力・閾値・出力先・レポート形式は変更しない。
新しい crate や依存は追加しない。
SPG-10 の履歴側とエンジン側の分離を完了とし、古い手書き FFI の置換や並列グラフの製品接続とは区別する。

## 回帰と基盤の保守

- `scripts/tests/test_engine_spike.py` は非既定 feature・モジュールの条件・計測専用の型と宣言・バイナリの条件を確認する。
  最初の非既定 feature のアサーションは変更前に失敗させた。製品の誤動作の再現とは区別する。
- 同じテストで実際の Cargo の既定 workspace の依存解決を読み、推移的な feature 統合でも spike が有効になっていないことを確認する。
- 既定と feature 付きの検査を分けること、両計測入口と必須契約の明示指定も検査する。
  既存のグラフの単独／並列比較と、実機なしの計測完走契約・負の対照を維持する。
- 実行方法と指定漏れの診断を開発・検査・デバッグ手順へ反映する。
  ログ形式・失敗の種類・出力先は増えないため、`doctor` と `debug-report` の収集範囲の変更は不要である。

S2 の過去の計測値は保持する。
実機の計測はユーザーの明示的な依頼がないため行わず、過去の値を今回の版の性能の合格へ移し替えない。
