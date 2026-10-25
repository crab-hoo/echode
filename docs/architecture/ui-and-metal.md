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
  リサイズでは前のフレームを見た目の左上に等倍で保ち、主スレッドで drawable のサイズも更新する。
  反転した View の固定位置は AppKit の変換に任せ、`contentsGravity` を上書きしない
  （[ADR 0144](../decisions/0144-unscaled-canvas-resizing.md)、[ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
  指定ヘッダ幅と現在の表示幅を分け、狭い layout を往復しても指定を失わない（[ADR 0144](../decisions/0144-unscaled-canvas-resizing.md)）。
- **デバイスビュー**:
  下部は選択トラックのデバイスを横に並べ、各列が画面内の行だけを作る全パラメーター一覧・検索・ピン留めを持つ。
  ピアノロールとデバイスは個別に開閉し、アレンジ・ピアノロール・固定高のデバイスの順に積む。
  アレンジ非表示の切替は各開閉と独立し、希望高と旧設定を保持する（[ADR 0146](../decisions/0146-independent-editor-stack.md)）。
  列と一覧はデバイスごとに文書を開いている間保持する（[ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md)）。
  FX 追加ボタンは最後の列の右側に置き、空のチェインでも同じ位置から候補を開く。
  列とインスペクターの名前クリックで独自 GUI を開く（[ADR 0132](../decisions/0132-helper-plugin-windows.md)）。
  開いている窓は名前の横のアイコンで示し、一時非表示と閉鎖を区別する。表示は文書内だけに保ち、保存・Undo に含めない
  （[ADR 0138](../decisions/0138-plugin-window-status.md)）。
  ヘッダも名前から独自 GUI を開く。先頭名と残りのチェインのメニューを FX 追加ボタンと分け、狭い表示では名前を隠す
  （[ADR 0139](../decisions/0139-track-header-fx-names.md)）。
  インスペクターの一覧末尾にも FX 追加を置き、空のチェインでも表示する。文書交換・終了中だけ無効にし、既存の候補へつなぐ
  （[ADR 0165](../decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
  列の開閉は文書内だけでデバイス ID ごとに保持し、保存・Undo・プラグイン通信を変更しない（[ADR 0137](../decisions/0137-device-disclosure.md)）。
  可変幅の累積を配置とドロップ判定に共有する。複数選択時の対象は未決である
  （[操作要件](../interaction-design.md#デバイスビューと-fx-一覧の役割)）。
- **単一 drawable のアレンジ表示**: トラックヘッダとタイムラインの縦の原点・行の高さは `ArrangePaneView` が単一所有し、左右の名前・背景・行境界・オブジェクト・音量・パン・M/S・ピークを 1 つの Metal フレームへ描く
  （[ADR 0050](../decisions/0050-arrange-viewport.md)、[ADR 0051](../decisions/0051-arrange-presentation.md)、[ADR 0107](../decisions/0107-mix-controls.md)）
- **状態分離**: 文書・選択・確定編集を `ProjectController`、配置・表示設定・下書きを `WorkspaceState`、画面操作を `WorkspaceSession` に分け、共通の `WorkspaceCommand` を使う
  （[ADR 0053](../decisions/0053-workspace-ui-foundation.md)）
- **固定した画面寸法**: 状態行は準備中だけ表示器を見せるが、その場所を常に確保する。
  再生ボタンは Play と Pause の広い方の幅を保つ。準備の開始と終了でペインの寸法を変えない
  （[ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
- **縦位置と表示の制限**: アレンジの `originTrack` は利用者の指定値とし、高さで制限したコピーだけを描画へ渡す。
  ペインの高さを戻すと指定位置へ戻る。`CanvasNavigationState.arrange` にも指定値を記憶する
  （[ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
- **ピアノロールのアクセシビリティ**: 描画の通知では要素を作らず、問い合わせ時に同じ描画版の要素を再利用する。
  フォーカスの通知に必要な要素だけを先に作る
  （[ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
- **編集順序と操作の排他**: `EditTurn` は編集を直列にするが、通常編集では画面全体を無効にしない。
  `ProjectController.busy` は文書の作成・交換・終了だけを表し、Undo 等の状態読込中のパラメータードラッグは `historyBusy` で制限する
  （[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
  再生・停止以外のトランスポートの要求は順番待ちで送り、操作子は `transportLocked`（文書なし・`busy`）だけで無効にする
  （[ADR 0160](../decisions/0160-ordered-transport-requests.md)）。
- **値の操作子**: ヘッダの音量・パン、クリップのハンドル、パラメーター欄は `ValueClicks` の同じ規則で押下を受ける。
  単クリックは値を変えず、ドラッグは押した値からの相対量、ダブルクリックは既定値の仮表示と間隔後の記録、3 回目は入力欄を開く。
  入力欄（`ValueEntryField`）は編集中のキーを先に受け、メニューの Space や文書の Undo へ渡さない
  （[ADR 0153](../decisions/0153-value-control-clicks-and-typed-entry.md)）。
- **フォルダの開閉**: 元のプロジェクト・索引を共有する表示用スナップショットを作り、同じ行対応をヘッダ・タイムライン・選択・ミックス・FX ドロップへ使う。
  開閉は表示設定とし、再生・保存・Undo を変えない（[ADR 0128](../decisions/0128-folder-disclosure-and-row-projection.md)）。
- **共通サンプルブラウザ**: 登録フォルダはウィンドウ設定に保持し、素材一覧は非同期で作る。
  入力イベントで文書・トラック・ライブ位置を確定し、既存の `audio.import` へ渡す。
  検索対象・除外・再読込の規則は [ADR 0122](../decisions/0122-sample-library-search-and-placement.md)。
  選択の変化で `WorkspaceSession.auditionSelection` が音声素材の試聴を要求し、編集の待ち行列を通さない
  （[ADR 0135](../decisions/0135-sample-audition.md)）。
  一覧は AppKit の表（`FileListTable`）で、押した時点で選択しフォーカスを取り、2 回目のクリックと Return で開く。
  試聴を変えるのは利用者のクリックとキーだけで、ブラウザ自身の選択の付け替えは鳴らさない（[ADR 0143](../decisions/0143-browser-click-and-audition-timing.md)）。
  一覧の下には選択した音声素材の全体の波形・長さを 1 つ表示する。全体は `SampleOverviews` が選択で読み、直近のものを保持する。
  読込み中だけ試聴の部分波形を使い、選択と違うパスの報告は使わない
  （[ADR 0151](../decisions/0151-streaming-sample-audition.md)、[ADR 0163](../decisions/0163-sample-overview.md)）。
  一覧にフォーカスがある時の Space は、一覧がワークスペースの「再生／停止」を呼ぶ（[ADR 0160](../decisions/0160-ordered-transport-requests.md)）。
  自動試聴のオン／オフと音量はウィンドウ設定に保持し、右矢印・ボタン・波形クリックの手動試聴は設定と独立して使える。
  ブラウザの表示・文書の交換で無音の出力を準備し、表示中は保持する（[ADR 0152](../decisions/0152-browser-audition-controls.md)）。
  制作向けの表示・検索と音声ファイル URL の位置指定ドロップは [ADR 0145](../decisions/0145-sample-browser-file-drops.md) に従う。
  表示行と現在のカメラから対象を確定し、最終行の下では `track.add` と取込みを 1 回の履歴にする。
- **P2 のアレンジ・ミックス編集**: 右ドラッグでクリップ選択、空白左ドラッグで編集区間選択、リージョン作成、複数選択ミキサー操作に対応する
  （[ADR 0117](../decisions/0117-workflow-interactions.md)、[ミックスの要件](../interaction-design.md#ミキサーの複数選択)）

### 13.2 Metal のレンダラー

- **専用描画スレッドと `CAMetalDisplayLink`**: `CanvasRenderer` がアレンジとピアノロールを共用し、専用 run loop 上で 3 つのフレーム枠（インスタンス用・波形用 Metal バッファ）を回す。
  停止中からの再開・一時停止・位置移動は世代を増やして 1 枚描いてから休止する
  （[ADR 0091](../decisions/0091-playhead-wake.md)、[ADR 0092](../decisions/0092-playhead-spike.md)、[ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)）
  レイヤーのサイズは主スレッドが所有する。入力と大きさの違う drawable にも同じ画素位置で現在の入力を描き、
  入力の外を背景に保ち（行ラベルも入力境界でクリップする）、その入力の版を消費せずに次のコールバックで正しい大きさへ描き直す
  （[ADR 0144](../decisions/0144-unscaled-canvas-resizing.md)、[ADR 0150](../decisions/0150-stale-drawable-frame.md)）。
  レイヤーの背景色も同じ表示面の背景色にそろえ、広がった余白を透明にしない。
  コールバックとウィンドウなしの製品回帰は `CanvasRenderer.submit` を共有し、描画・版の更新・休止状態まで同じ経路を通す。
  検査で省くのは表示リンクの起動と画面への提出だけで、GPU 完了待ちは検査の読み戻し側に限定する。
- **可視要素とキャッシュ**: 見えているトラック・配置・ノートを単一の可視列として問い合わせ、描画・ヒットテスト・VoiceOver で共用する
  （[ADR 0045](../decisions/0045-visible-elements.md)）。
  アレンジのノートは中身ごとの音域（最低 12 半音）で高い音ほど上へ置く。
  音域は中身の索引に保持し、配置ごとに縦変換を準備して可視ノートだけを処理する。
  上下 14% の帯を避け、1 pt を下限にしたノートの矩形を描画と VoiceOver で共有する
  （[ADR 0165](../decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
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
停止中の最初の表示・再生で出力とヘルパーを準備する間も、ワーカーは後続の編集を続け、デバイス要求だけが投入順に待つ
（[ADR 0136](../decisions/0136-output-opening-off-worker.md)）。
Swift も `AudioState.opening` を受け取り、未取得のデバイス列は出力準備と一覧の応答待ちを分けて表示する
（[ADR 0141](../decisions/0141-device-output-preparation-status.md)）。
製品アプリは開いた直後に出力を準備するため、その間の未取得の列も出力準備を表示する
（[ADR 0147](../decisions/0147-output-prewarm-and-background-media.md)）。
出力を開いた後に新しいヘルパーが必要な編集は、まだワーカーで起動を待つ。

### 13.4 トラックパッドとジェスチャー

- **ピンチズーム**: 既定は画面の主軸（横＝時間）、Option で反対軸（高さ／音高）、Shift で縦横同率とする。
  P2 のピアノロール時間軸ピンチはポインター位置の時刻を固定する
- **スクロールと触覚**: 2 本指スクロールはピクセル移動量をそのまま使い、マウスホイールはトラック／音高と拍に換算する。
  触覚フィードバックは `NSHapticFeedbackManager` の 3 パターンを使う
- **再生ヘッドの追従**: 両エディタは範囲外へ出た再生ヘッドへ時間の原点をページ送りし、横方向が優勢なスクロールで追従を止める。
  判定は主スレッドの `PlayheadFollower` が原子的な位置を読み、追従中の再生と transport 変化の直後だけ表示リンクを動かす。
  押下中のルーラー・ノート・クリップ操作の間は保留し、追従の値は保存しない
  （[ADR 0159](../decisions/0159-playhead-follow.md)）

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
  - **ルーラーとループ**: クリックで開始位置記憶、ドラッグでループ範囲指定（[ループ再生](../interaction-design.md#ループ再生)）。
    両エディタはクリックの解放時に開始位置を記憶し、ドラッグでは範囲だけを 1 回確定する。
    各グリッドへの吸着と Shift、逆向き・取消、有効／無効の帯と範囲欄の同期を持つ。
    アレンジは単一 Metal フレームの上端 24 pt を使い、行の入力と描画はその下へそろえる。
    既存端は上端のグリップで反対端を固定して調整する。交差を制限し、外部の範囲・オンオフ変更でも取り消す
    （[ADR 0158](../decisions/0158-ruler-loop-edge-handles.md)）。
  - **ノートの入力と記憶長**: シングルクリックで記憶長入力、新規ドラッグで長さ・音高変更。記憶長は編集セッション内で全トラック共有し、Undo で戻す
    （[ノートの入力](../interaction-design.md#ノートの入力)）
  - **移動・伸縮・右クリックメニュー**: 時間グリッド吸着（Shift で解除）、複数選択の相対移動と一括削除
    （[ノートの選択](../interaction-design.md#ノートの選択)）
  - **ベロシティ編集**: ノート上の Option＋上下ドラッグと下部ベロシティ欄の双方向接続、複数選択の同量増減
    （[ベロシティの編集](../interaction-design.md#ベロシティの編集)）
  - **トラックセレクターとドラム入力**: 右端パネルで「表示」「編集」「ソロ」「ミュート」を独立切替し、フォルダ型ドラムラック（§8.5）の親 MIDI を音色名行で入力する
    （[トラックセレクター](../interaction-design.md#トラックセレクター)）
- **MPE・アーティキュレーション・コード（後続 Phase）**: MPE マイクロレーン、Articulation Set、コード推定とコードトラックに対応する
