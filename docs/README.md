# 資料の入口

AI エージェントはまず [AGENTS.md](../AGENTS.md) を読む。
開始は `make agent-context`（個別確認は `make doctor` と `make verify-plan`）、終了は `make verify`。
この索引は資料の役割を示し、検査の選択条件は持たない（[ADR 0094](decisions/0094-agent-development-foundation.md)、[ADR 0119](decisions/0119-agent-token-efficiency.md)）。

## 作業で使う資料

- [現在の範囲と制約](status.md): 実装済み、未接続、未検証を区別し、Phase 1 の完了と Phase 2 の進捗を確認する入口。
- [P1完了条件の最終照合](reviews/2026-10-17-phase1-acceptance.md): Phase 1 の 4 条件の製品経路・自動回帰・聴感確認、将来機能との境界。
- [Phase 2 の進め方](phase2.md): Phase 2 の実装単位（P2-01〜P2-09）の順番と各単位の受入条件。
- [制作ワークフローとインタラクションの検討](interaction-design.md): 制作フローの操作要件と未決事項の正本（[ADR 0117](decisions/0117-workflow-interactions.md)）。
- [開発手順](development.md): 作業の順番、[コードの所在](development.md#コードの所在)、回帰テストの追加方法、[基盤の保守](development.md#基盤の保守を同じ作業で行う)。
- [エージェントの情報取得と実行](agent-workflow.md): 一括の開始診断、根拠付きの節・行の読込、実行と引継ぎの原則。
- [検査手順](verification.md): 自動検査と実機検査、見送り、結果の有効期限、フックと CI。必須契約の一覧は [scripts/verification.json](../scripts/verification.json) が正本。
- [デバッグ手順](debugging.md): 環境・ソース・アプリ・検査を切り分け、再現と診断資料を残す手順。
- [PLAN](PLAN.md): 製品の方針、ライセンス原則（[§2.1](PLAN.md#21-ライセンス方針の影響)）、ロードマップと完了条件。
- [ARCHITECTURE](ARCHITECTURE.md): 全体構成と設計原則の目次。領域別の詳細は [docs/architecture/](architecture/) 配下の分割ファイルを参照する。
  - [モデル・時間・Git 履歴](architecture/model-and-history.md)（§4〜§6）
  - [オーディオエンジン・DSP・解析](architecture/audio-engine.md)（§7、§8、§11）
  - [プラグイン隔離・内蔵デバイス](architecture/plugins-and-devices.md)（§9、§10）
  - [GPU・UI・Metal・ピアノロール](architecture/ui-and-metal.md)（§12〜§14）
  - [API / MCP・品質保証・ビルド配布](architecture/api-qa-and-distribution.md)（§15〜§17）
- [ADR](decisions/): 設計判断の記録。番号は時系列であり、置き換えた判断は後の ADR を参照する。
- [スパイク](spikes/): 条件付きの測定結果（S1〜S5）。測定当時の機器・版・入力の範囲に限る。

## 監査の読み順

作業する領域に関係する監査と ADR だけを読む。全項目を毎回通読しない。
監査は発見時点の証拠を残すため、過去の「失敗」「未実施」を書き換えず、現在の合否は `target/verification/report.md` で確認する。

### 1. 全体横断・Phase 1 最終照合

- [2026-10-08 初期監査](reviews/2026-10-08-audit.md): HIS / MOD / ENG / PLG / API / UI / DEV の一覧と未着手項目（`audit_*` テスト）。
- [2026-10-13 境界監査](reviews/2026-10-13-audit.md)、[Phase 1 専門監査](reviews/2026-10-13-phase1-pro-audit.md): AUD-01〜13、PRO-01〜17。
- [修正指示](reviews/2026-10-14-phase1-fix-instructions.md)、[修正後監査](reviews/2026-10-14-phase1-fix-reaudit.md): 修正条件と再現結果。
- [2026-10-17 境界再監査](reviews/2026-10-17-boundary-reaudit.md)、[P1 最終照合](reviews/2026-10-17-phase1-acceptance.md): Phase 1 完了判定と境界回帰（[ADR 0108](decisions/0108-phase1-loop-readiness.md)、[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。

### 2. オーディオエンジン・リアルタイム・再生

- ステレオ・ループ・フェーダー補間: [ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)、[ADR 0108](decisions/0108-phase1-loop-readiness.md)。
- 再生準備の取消・排他グラフ・停止減衰・非ブロック後片付け: [ADR 0097](decisions/0097-cancellable-retained-transport.md)、[ADR 0101](decisions/0101-live-output-discontinuities.md)、[ADR 0102](decisions/0102-exclusive-audio-graph.md)、[ADR 0103](decisions/0103-live-stop-waveform.md)、[ADR 0104](decisions/0104-stop-output-diagnostics.md)、[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)。
- 停止中のプラグイン処理・自然な余韻・編集時の全体暗転の解消: [ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)。
- 実機計測の完走条件: [ADR 0096](decisions/0096-measurement-completion.md)。

### 3. プラグイン隔離・サンドボックス・Workgroup

- CLAP バンドル・停止時リセット・自然な発音: [ADR 0093](decisions/0093-product-contract-gate.md)、[ADR 0099](decisions/0099-transport-plugin-state.md)、[ADR 0106](decisions/0106-natural-plugin-onsets.md)。
- macOS ヘルパー通知・Workgroup 引き継ぎ: [2026-10-17 macOS 境界監査](reviews/2026-10-17-macos-helper-audit.md)、[ADR 0111](decisions/0111-macos-helper-notices.md)、[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)。
- フォルダ型ドラムラックと FX チェイン: [ADR 0116](decisions/0116-folder-drum-rack.md)、[ADR 0118](decisions/0118-track-fx-chain-editing.md)。
- 選択トラックへの FX 一括追加: [ADR 0124](decisions/0124-selected-track-fx-batches.md)。
- 同梱 Sine Pad の明示的な CLAP 化・音源なしの無音: [ADR 0123](decisions/0123-explicit-sine-pad-plugin.md)。
- CLAP のホストバイパス・保存形式 4: [ADR 0120](decisions/0120-device-bypass.md)。
- CLAP のオフライン化・保存形式 5: [ADR 0125](decisions/0125-device-offline.md)。
- CLAP のパラメーター編集・停止中のヘルパー起動・状態の Undo: [ADR 0130](decisions/0130-clap-parameter-editing.md)。
- 横並びのデバイスビュー・画面内の行だけの一覧・検索とピン留め・保存形式 6・ワーカー外のデバイス要求: [ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)。
- GUI の一時非表示・ループ変更での寿命、GUI フォーカス中のキー中継、末尾の FX 追加:
  [ADR 0134](decisions/0134-plugin-window-lifetime-and-shortcuts.md)。
- ヘルパー所有のプラグインウィンドウ・フォーカス・プラグイン側の変更の中継と単一 Undo: [ADR 0132](decisions/0132-helper-plugin-windows.md)。

### 4. UI・Metal 描画・入力・Swift 橋渡し

- フォルダ開閉と表示行の保持: [ADR 0128](decisions/0128-folder-disclosure-and-row-projection.md)。

- Computer Use 監査と再調査: [Computer Use 監査](reviews/2026-10-14-computer-use.md)、[取得エラー再調査](reviews/2026-10-14-computer-use-reinvestigation.md)、[同一デスクトップ再監査](reviews/2026-10-14-computer-use-same-desktop.md)。
- ペイン境界・ミックス UI・高リフレッシュ描画・可視列キャッシュ: [ADR 0098](decisions/0098-pane-render-boundaries.md)、[ADR 0107](decisions/0107-mix-controls.md)、[2026-10-17 Apple Silicon 監査](reviews/2026-10-17-apple-silicon-audit.md)、[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)、[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)。

### 5. モデル・Git 履歴・CLI / MCP

- 配置境界・ピーク部分読出・状態取り込み: [ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)、[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)。
- Phase 2 ヘッドレス MCP: [ADR 0115](decisions/0115-phase2-mcp.md)。
- 選択位置へのトラック挿入と末尾追加: [ADR 0127](decisions/0127-positioned-track-insertion.md)。
- トラックの通常複製と導出 ID: [ADR 0129](decisions/0129-track-duplication.md)。

### 6. ビルド・対象プラットフォーム・検査基盤

- macOS 27 / M4 基準と最適化監査: [2026-10-17 macOS 27 最適化監査](reviews/2026-10-17-macos27-optimization-audit.md)、[ADR 0112](decisions/0112-m4-minimum.md)、[ADR 0114](decisions/0114-m4-build-baseline.md)。
- 検査基盤とトークン効率化: [ADR 0093](decisions/0093-product-contract-gate.md)、[ADR 0094](decisions/0094-agent-development-foundation.md)、[ADR 0100](decisions/0100-scoped-verification.md)、[ADR 0119](decisions/0119-agent-token-efficiency.md)。
- 一括開始診断と根拠付きの節・行の読込: [ADR 0126](decisions/0126-agent-context-and-source-reading.md)。
- 起動復旧の終了待ち・Darwin errno の検査ランタイム修正: [ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)。
