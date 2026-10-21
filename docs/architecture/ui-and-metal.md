# UI・Metal 描画・橋渡し・ピアノロールアーキテクチャ（§12〜§14）

- 全体索引: [../ARCHITECTURE.md](../ARCHITECTURE.md)
- 操作要件の正本: [../interaction-design.md](../interaction-design.md)
- 現在の実装範囲: [../status.md](../status.md)

## 12. GPU（Metal）の使い方

| 用途 | 処理する場所 | リアルタイムの経路に入るか |
|---|---|---|
| UI の描画 | Swift のレンダラー | 入らない |
| 波形ピーク | CPU（`echode-history`）。描画は Swift のシェーダー | 入らない |
| スペクトログラム、テンポ・キー・トランジェントの解析 | `echode-gpu`（未着手） | 入らない |
| マルチトラック EQ に重ねる他トラックのスペクトラム | `echode-gpu` | 入らない（表示用） |
| 先読み中のトラックの重い内蔵 DSP | `echode-gpu` | 先読みの余裕の中でだけ使う（間に合わなければ CPU） |
| バウンスやフリーズの時の重い内蔵 DSP | `echode-gpu` | 入らない |

ライブ入力が通る経路では GPU と `Accelerate`（vDSP）を使わず、製品のミックスとピーク生成は確保やロックをしないスカラー処理で行う
（[ADR 0058](../decisions/0058-waveform-peaks.md)、[ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)）。
M4 以降への対象変更により、SME / SME2 を利用する既存実装と `Accelerate` を非 RT の解析・オフライン処理の比較候補に含める。
GPU は M4 の Apple9 ファミリーを基準とする（[ADR 0112](../decisions/0112-m4-minimum.md)）。

## 13. UI 層

### 13.1 画面の構成

- **ペイン構成**: 左ペイン（インスペクター）、中央（アレンジ画面）、中央下部（MIDI エディタ・オーディオエディタ・デバイスビュー）、右ペイン（ブラウザ・マスター）。
  ペインの親が先に縮んでも隣の領域へはみ出さないよう AppKit の親・表示面で明示的に切る
  （[ADR 0098](../decisions/0098-pane-render-boundaries.md)）
- **デバイスビュー**:
  下部は選択トラックのデバイスを横に並べ、各列が画面内の行だけを作る全パラメーター一覧・検索・ピン留めを持つ。
  切替は「ピアノロール」「デバイス」「両方（上下）」。列と一覧はデバイスごとに文書を開いている間保持する
  （[ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md)）。
  列とインスペクターの名前クリックで独自 GUI を開く（[ADR 0132](../decisions/0132-helper-plugin-windows.md)）。
  展開・折りたたみ、複数選択時の対象は未実装・未決である
  （[操作要件](../interaction-design.md#デバイスビューと-fx-一覧の役割)）。
- **単一 drawable のアレンジ表示**: トラックヘッダとタイムラインの縦の原点・行の高さは `ArrangePaneView` が単一所有し、左右の名前・背景・行境界・オブジェクト・音量・パン・M/S・ピークを 1 つの Metal フレームへ描く
  （[ADR 0050](../decisions/0050-arrange-viewport.md)、[ADR 0051](../decisions/0051-arrange-presentation.md)、[ADR 0107](../decisions/0107-mix-controls.md)）
- **状態分離**: 文書・選択・確定編集を `ProjectController`、配置・表示設定・下書きを `WorkspaceState`、画面操作を `WorkspaceSession` に分け、共通の `WorkspaceCommand` を使う
  （[ADR 0053](../decisions/0053-workspace-ui-foundation.md)）
- **編集順序と操作の排他**: `EditTurn` は編集を直列にするが、通常編集では画面全体を無効にしない。
  `ProjectController.busy` は文書の作成・交換・終了だけを表し、Undo 等の状態読込中のパラメータードラッグは `historyBusy` で制限する
  （[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
- **フォルダの開閉**: 元のプロジェクト・索引を共有する表示用スナップショットを作り、同じ行対応をヘッダ・タイムライン・選択・ミックス・FX ドロップへ使う。
  開閉は表示設定とし、再生・保存・Undo を変えない（[ADR 0128](../decisions/0128-folder-disclosure-and-row-projection.md)）。
- **共通サンプルブラウザ**: 登録フォルダはウィンドウ設定に保持し、素材一覧は非同期で作る。
  入力イベントで文書・トラック・ライブ位置を確定し、既存の `audio.import` へ渡す。
  検索対象・除外・再読込の規則は [ADR 0122](../decisions/0122-sample-library-search-and-placement.md)、単発試聴は未実装。
- **P2 のアレンジ・ミックス編集**: 右ドラッグでクリップ選択、空白左ドラッグで編集区間選択、リージョン作成、複数選択ミキサー操作に対応する
  （[ADR 0117](../decisions/0117-workflow-interactions.md)、[ミックスの要件](../interaction-design.md#ミキサーの複数選択)）

### 13.2 Metal のレンダラー

- **専用描画スレッドと `CAMetalDisplayLink`**: `CanvasRenderer` がアレンジとピアノロールを共用し、専用 run loop 上で 3 つのフレーム枠（インスタンス用・波形用 Metal バッファ）を回す。
  停止中からの再開・一時停止・位置移動は世代を増やして 1 枚描いてから休止する
  （[ADR 0091](../decisions/0091-playhead-wake.md)、[ADR 0092](../decisions/0092-playhead-spike.md)、[ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)）
- **可視要素とキャッシュ**: 見えているトラック・配置・ノートを単一の可視列として問い合わせ、描画・ヒットテスト・VoiceOver で共用する
  （[ADR 0045](../decisions/0045-visible-elements.md)）。
  入力の版とカメラが変わらない間は `CanvasGeometryCache` で再利用し、再生位置とピークだけを毎フレーム読む
  （[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)）

### 13.3 Rust と Swift の橋渡し

| 経路 | 方式 | 用途 |
|---|---|---|
| 命令 | `cbindgen` で生成する C ABI と Swift の非同期関数・完了コールバック | アクションの実行、問い合わせ、変更の通知 |
| データ | 所有する C ABI のスナップショットハンドルと読み取り専用ポインタ | ノート、クリップ、波形を複製せずに読む |
| 計測値 | 共有メモリ（アトミック変数） | メーター、プレイヘッド、CPU 負荷 |

命令とデータを C ABI にそろえ、`EchodeBridge` が非同期関数・エラー・所有権へ包む（[ADR 0044](../decisions/0044-swift-bridge.md)）。
編集 JSON は単一のアクション、または単一 Undo のアクション配列を受ける。
配列は同じ文書ワーカーの `DocumentHost::apply` で全体を確定・巻戻しする
（[ADR 0124](../decisions/0124-selected-track-fx-batches.md)）。
命令は制御スレッドへ送ってすぐ戻り、ロックを保持したまま Swift を呼ばない。
配列を借りる同期クロージャーの間はハンドルを保持し、ポインタを外へ出さず `await` しない。

GUI 応答性の追加要件は [処理中も作業を続ける](../interaction-design.md#処理中も作業を続ける)を参照する。
非同期の C ABI があることだけで、後続の編集が長いプラグイン待ちを追い越せるとは判断しない。
デバイス要求の往復は文書ワーカーの外のデバイスレーンで待ち、取り込んだ状態の記録だけをワーカーの順序へ戻す。
一覧の応答は文書・デバイス・要求世代で照合し、Undo 後の読み直しは編集の手番の外で行う。
パラメーターのドラッグを止めるのは状態を読み込みうる履歴操作の間だけで、通常の編集中は止めない。
停止中の最初の表示で出力とヘルパーを開く処理はまだワーカーで待つ（[ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md)）。

### 13.4 トラックパッドとジェスチャー

- **ピンチズーム**: 既定は画面の主軸（横＝時間）、Option で反対軸（高さ／音高）、Shift で縦横同率とする。
  P2 のピアノロール時間軸ピンチはポインター位置の時刻を固定する
- **スクロールと触覚**: 2 本指スクロールはピクセル移動量をそのまま使い、マウスホイールはトラック／音高と拍に換算する。
  触覚フィードバックは `NSHapticFeedbackManager` の 3 パターンを使う

### 13.5〜13.7 キーボード MIDI・アクセシビリティ・自動色分け

- **キーボード MIDI と疑似 MPE（§13.5）**: QWERTY 鍵盤演奏と、トラックパッド圧力・XY 移動による疑似 MPE に対応する
- **アクセシビリティ（§13.6）**: 可視の子と論理的な子を分けて VoiceOver に公開し、全操作をアクション経由で実行できるようにする
  （[ADR 0045](../decisions/0045-visible-elements.md)）
- **トラックの自動色分け（§13.7）**: トラック名・楽器種別・フォルダ階層から系統を推定し、OKLab で知覚色差を確保する

## 14. MIDI とピアノロール

- **Phase 1 の実装**: `PianoRollView.swift`、`PianoGeometry.swift`、`PianoFrame.swift` でトラック全体の発音ノートを表示し、ドラッグ中は Swift の幾何情報だけをプレビューして離した時に `note.edit` を 1 回送る。
  左からディグリー、鍵盤、ノートの順に同じ不変入力から 1 つの Metal フレームへ描き、物理キーコードと VoiceOver 操作に対応する
  （[ADR 0052](../decisions/0052-piano-keyboard.md)）
- **P2 のピアノロール操作**:
  - **ルーラーとループ**: クリックで開始位置記憶、ドラッグでループ範囲指定（[ループ再生](../interaction-design.md#ループ再生)）
  - **ノートの入力と記憶長**: シングルクリックで記憶長入力、新規ドラッグで長さ・音高変更。記憶長は編集セッション内で全トラック共有し、Undo で戻す
    （[ノートの入力](../interaction-design.md#ノートの入力)）
  - **移動・伸縮・右クリックメニュー**: 時間グリッド吸着（Shift で解除）、複数選択の相対移動と一括削除
    （[ノートの選択](../interaction-design.md#ノートの選択)）
  - **ベロシティ編集**: ノート上の Option＋上下ドラッグと下部ベロシティ欄の双方向接続、複数選択の同量増減
    （[ベロシティの編集](../interaction-design.md#ベロシティの編集)）
  - **トラックセレクターとドラム入力**: 右端パネルで「表示」「編集」「ソロ」「ミュート」を独立切替し、フォルダ型ドラムラック（§8.5）の親 MIDI を音色名行で入力する
    （[トラックセレクター](../interaction-design.md#トラックセレクター)）
- **MPE・アーティキュレーション・コード（後続 Phase）**: MPE マイクロレーン、Articulation Set、コード推定とコードトラックに対応する
