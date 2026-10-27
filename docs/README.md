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

- [2026-10-25 現行コード監査](reviews/2026-10-25-code-audit.md): CUR-01〜04。
  不正 PCM の panic、素材終端・テンポ変化での波形誤表示、API 応答の書込み待ちを再現し、後日対応を追記した。

- [2026-10-08 初期監査](reviews/2026-10-08-audit.md): HIS / MOD / ENG / PLG / API / UI / DEV の一覧と未着手項目（`audit_*` テスト）。
- [2026-10-13 境界監査](reviews/2026-10-13-audit.md)、[Phase 1 専門監査](reviews/2026-10-13-phase1-pro-audit.md): AUD-01〜13、PRO-01〜17。
- [修正指示](reviews/2026-10-14-phase1-fix-instructions.md)、[修正後監査](reviews/2026-10-14-phase1-fix-reaudit.md): 修正条件と再現結果。
- [2026-10-17 境界再監査](reviews/2026-10-17-boundary-reaudit.md)、[P1 最終照合](reviews/2026-10-17-phase1-acceptance.md): Phase 1 完了判定と境界回帰（[ADR 0108](decisions/0108-phase1-loop-readiness.md)、[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。

### 2. オーディオエンジン・リアルタイム・再生

- ステレオ・ループ・フェーダー補間: [ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)、[ADR 0108](decisions/0108-phase1-loop-readiness.md)。
- 再生準備の取消・排他グラフ・停止減衰・非ブロック後片付け: [ADR 0097](decisions/0097-cancellable-retained-transport.md)、[ADR 0101](decisions/0101-live-output-discontinuities.md)、[ADR 0102](decisions/0102-exclusive-audio-graph.md)、[ADR 0103](decisions/0103-live-stop-waveform.md)、[ADR 0104](decisions/0104-stop-output-diagnostics.md)、[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)。
- 停止中のプラグイン処理・自然な余韻・編集時の全体暗転の解消: [ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)。
- 実機計測の完走条件: [ADR 0096](decisions/0096-measurement-completion.md)。
- 文書ワーカーから独立したサンプルの単発試聴・専用 IOProc: [ADR 0135](decisions/0135-sample-audition.md)。
  有限の先読み・連続 SRC と同じ読込みからの部分波形: [ADR 0151](decisions/0151-streaming-sample-audition.md)。
  表示中の出力保持・音量・自動試聴の切替と手動の開始位置: [ADR 0152](decisions/0152-browser-audition-controls.md)。
  一覧を押した時点の選択、利用者の操作だけで変わる試聴、要求ごとの各段の時刻: [ADR 0143](decisions/0143-browser-click-and-audition-timing.md)。
- 出力とヘルパーの準備を文書ワーカーの外で行う・機器なしの検査用出力: [ADR 0136](decisions/0136-output-opening-off-worker.md)。
- 開いた出力の置換ヘルパー・共有領域の準備と後続編集の集約: [ADR 0142](decisions/0142-background-plugin-graph-preparation.md)。
- プロジェクトを開いた時の出力準備・停止中の編集の素材の別スレッド読込み・読込み中の再生の保留: [ADR 0147](decisions/0147-output-prewarm-and-background-media.md)。
- 記憶した再生開始位置・余韻を保つ停止での復帰・一時停止・`transport.json`・ピアノロールのルーラー: [ADR 0155](decisions/0155-remembered-playback-start.md)。
- ピアノロールのルーラードラッグの範囲指定・クリックとの分離と、ループ前からの進入・終端以降の通常進行:
  [ADR 0156](decisions/0156-ruler-loop-range-and-entry.md)。
- アレンジのルーラー・両エディタの開始位置と範囲共有・行領域の上端: [ADR 0157](decisions/0157-arrange-ruler.md)。
- 両ルーラーの既存端の調整・つかむ位置と反対端の保持・外部変更による取消: [ADR 0158](decisions/0158-ruler-loop-edge-handles.md)。
- 両エディタの再生ヘッドへのページ送り・手動の時間スクロールでの解除・保存しない追従の値: [ADR 0159](decisions/0159-playhead-follow.md)。

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

- 下部の個別開閉・固定高のデバイス・アレンジ非表示と旧設定の移行: [ADR 0146](decisions/0146-independent-editor-stack.md)。
- 操作の要望の照合（リサイズ時の伸縮、ブラウザ・クリップの D&D、既定値と値入力のクリック、試聴、下部の積み方）: [2026-10-21 操作の要望の照合](reviews/2026-10-21-ui-feedback-audit.md)。
- 2026-10-25 の操作の報告（開始位置の変更時の暗転、一覧の Space、ブラウザの波形、置いた音声のフェード、FX 候補、ノートの終端からの再生）と追加の要望:
  [2026-10-25 操作の報告の照合](reviews/2026-10-25-user-feedback.md)。
  対応は [ADR 0160](decisions/0160-ordered-transport-requests.md)〜[ADR 0164](decisions/0164-note-end-chase.md)、
  インスペクターの FX 追加とアレンジの音高表示の要件は [ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)。
- 制作向けファイルの一覧と音声の位置指定ドロップ・トラック追加を伴う取込みの単一履歴: [ADR 0145](decisions/0145-sample-browser-file-drops.md)。
- デバイス列での初回出力準備と一覧の応答待ちの区別: [ADR 0141](decisions/0141-device-output-preparation-status.md)。
- パラメーター一覧の失敗からの列内の再試行・連打の集約: [ADR 0140](decisions/0140-device-parameter-list-retry.md)。
- 独自ウィンドウの寿命通知と、デバイス列・インスペクターの窓アイコン: [ADR 0138](decisions/0138-plugin-window-status.md)。
- トラックヘッダの名前と FX 追加入口・残りのチェインのメニュー: [ADR 0139](decisions/0139-track-header-fx-names.md)。
- フォルダ開閉と表示行の保持: [ADR 0128](decisions/0128-folder-disclosure-and-row-projection.md)。

- デバイス列の開閉・文書内の表示状態・可変幅の FX ドロップ: [ADR 0137](decisions/0137-device-disclosure.md)。
- Computer Use 監査と再調査: [Computer Use 監査](reviews/2026-10-14-computer-use.md)、[取得エラー再調査](reviews/2026-10-14-computer-use-reinvestigation.md)、[同一デスクトップ再監査](reviews/2026-10-14-computer-use-same-desktop.md)。
- ペイン境界・ミックス UI・高リフレッシュ描画・可視列キャッシュ: [ADR 0098](decisions/0098-pane-render-boundaries.md)、[ADR 0107](decisions/0107-mix-controls.md)、[2026-10-17 Apple Silicon 監査](reviews/2026-10-17-apple-silicon-audit.md)、[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)、[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)。
- 値の操作子のクリック規則・入力欄・`text_to_value` の文字入力: [ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md)。
- ピアノロールの単一ノートのドラッグでの Shift 吸着解除・最短 1 tick と解放時の確定:
  [ADR 0167](decisions/0167-piano-note-drag-snapping.md)。
- ノートの音価・位置の編集で固定クリップも全長まで広げ、確定表示と履歴へ反映する:
  [ADR 0168](decisions/0168-note-edit-clip-growth.md)。
- ピアノロールの空白ドラッグによる追加・解放時の単一確定・取消と新規の最短長:
  [ADR 0171](decisions/0171-piano-blank-drag-insertion.md)。
  単クリックの入力・共有する入力長・Undo / Redo と再オープン時の初期化: [ADR 0172](decisions/0172-piano-click-and-input-length.md)。
- ピアノロールの右ドラッグ枠選択・Command の追加と解除・右メニューと一括削除:
  [ADR 0176](decisions/0176-piano-note-selection-and-deletion.md)。
- 単一ノートの Option＋上下ドラッグ・幾何を保つベロシティ専用アクションと全トラック共通の入力値:
  [ADR 0173](decisions/0173-piano-note-velocity.md)。
  下部の棒と全音高の時刻内表示・共通仮表示・単一確定・読み上げの操作: [ADR 0175](decisions/0175-piano-velocity-lane.md)。
- 自動配置の中身の小節基準と、小節未満の移動・複製での自動状態の保持:
  [ADR 0170](decisions/0170-content-relative-auto-bars.md)。
  ノートの所属替え・固定窓の小節拡張・隣のクリップを壁にする残りの見直し案（草案・未採用）:
  [ADR 0169](decisions/0169-clip-growth-follows-notes-and-stops-at-walls.md)。
- ピアノロールのトラック切替・再生成で共有する時間・音域・倍率と、別文書での初期化:
  [ADR 0174](decisions/0174-shared-piano-viewport.md)。
- ペインのリサイズ中の伸縮とヘッダ幅の保持: [ADR 0144](decisions/0144-unscaled-canvas-resizing.md)。
- 反転した表示面の固定位置と状態行・再生ボタンの寸法: [ADR 0149](decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)。
- 入力と大きさの違う drawable への描画と、広がった余白の背景: [ADR 0150](decisions/0150-stale-drawable-frame.md)。
- 別作業ツリーのサイズ不一致 drawable 修正の評価: [2026-10-24 omp-dual の実装監査](reviews/2026-10-24-omp-dual-audit.md)。
- 再生開始・ピアノロールの表示・ペインのリサイズ時のがくつき（上下の固定位置の誤り、状態行の高さ、作り直しの負荷）:
  [2026-10-22 GUI のがくつきの監査](reviews/2026-10-22-gui-jank-audit.md)。

### 5. モデル・Git 履歴・CLI / MCP

- 配置境界・ピーク部分読出・状態取り込み: [ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)、[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)。
- Phase 2 ヘッドレス MCP: [ADR 0115](decisions/0115-phase2-mcp.md)。
- 選択位置へのトラック挿入と末尾追加: [ADR 0127](decisions/0127-positioned-track-insertion.md)。
- トラックの通常複製と導出 ID: [ADR 0129](decisions/0129-track-duplication.md)。
- クリップの移動・複製移動と置き換えでの着地（削除・切詰め・分割）: [ADR 0154](decisions/0154-clip-move-and-duplicate.md)。

### 6. ビルド・対象プラットフォーム・検査基盤

- macOS 27 / M4 基準と最適化監査: [2026-10-17 macOS 27 最適化監査](reviews/2026-10-17-macos27-optimization-audit.md)、[ADR 0112](decisions/0112-m4-minimum.md)、[ADR 0114](decisions/0114-m4-build-baseline.md)。
- 検査基盤とトークン効率化: [ADR 0093](decisions/0093-product-contract-gate.md)、[ADR 0094](decisions/0094-agent-development-foundation.md)、[ADR 0100](decisions/0100-scoped-verification.md)、[ADR 0119](decisions/0119-agent-token-efficiency.md)。
- 一括開始診断と根拠付きの節・行の読込: [ADR 0126](decisions/0126-agent-context-and-source-reading.md)。
- 検査の実行時間（反例のコピーの保持・契約ごとの再利用・定義単位の選択）: [ADR 0148](decisions/0148-verification-pipeline-speed.md)。
- 終了コードの保証・実行予定だけの環境事前確認・依存順序と失敗検出遅延: [ADR 0166](decisions/0166-verification-failure-detection.md)。
- 起動復旧の終了待ち・Darwin errno の検査ランタイム修正: [ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)。
