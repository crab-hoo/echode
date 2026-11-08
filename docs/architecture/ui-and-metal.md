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

#### ペインとデバイスビュー

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
  可変幅の累積を配置とドロップ判定に共有する。複数選択時もフォーカスしたトラックを対象にする
  （[操作要件](../interaction-design.md#デバイスビューと-fx-一覧の役割)、[ADR 0276](../decisions/0276-priority-order-and-delegated-decisions.md)）。

#### 表示と文書の状態

- **単一 drawable のアレンジ表示**: トラックヘッダとタイムラインの縦の原点・行の高さは `ArrangePaneView` が単一所有し、左右の名前・背景・行境界・オブジェクト・音量・パン・M/S・ピークを 1 つの Metal フレームへ描く
  （[ADR 0050](../decisions/0050-arrange-viewport.md)、[ADR 0051](../decisions/0051-arrange-presentation.md)、[ADR 0107](../decisions/0107-mix-controls.md)）
- **状態分離**: 文書・選択・確定編集を `ProjectController`、配置・表示設定・下書きを `WorkspaceState`、画面操作を `WorkspaceSession` に分け、共通の `WorkspaceCommand` を使う
  （[ADR 0053](../decisions/0053-workspace-ui-foundation.md)）
- **書出し**: 設定シートを閉じた後はウィンドウ所有の `ExportJob` が状態を保持し、文書交換で対象を差し替えない。
  記録済みの曲を準備した後、同梱 CLI を別プロセスで実行する（[ADR 0254](../decisions/0254-gui-snapshot-export.md)）。
  出力先の親は準備前に解決し、元パッケージ内を拒否した固定パスを CLI へ渡す。外向きの葉リンクは参照先でなくリンク自身を置換する（[ADR 0260](../decisions/0260-pinned-gui-export-destination.md)）。
  CLI の任意の進捗を受け、率と仕上げ中を表示する。完了通知だけでは成功にしない（[ADR 0255](../decisions/0255-export-progress.md)）。
  診断は 64 KiB 以内なら全体、それ以上は先頭・末尾各 32 KiB と省略表示を保持し、最後の完了・失敗情報を落とさない（[ADR 0268](../decisions/0268-bounded-export-diagnostics.md)）。
  途中取消は専用の制御パイプで要求し、CLI が公開前に認めた時だけ取消済みにする（[ADR 0256](../decisions/0256-export-cancellation.md)）。
  前回の設定は文書ワーカー経由で `export.json` へ保持し、移動・コピー先でも読み戻す。モデル・履歴には入れない（[ADR 0257](../decisions/0257-portable-export-settings.md)）。
  数値欄は文字列の下書きを保持し、有効欄全体の検証に通った設定だけを開始と CLI 引数へ渡す（[ADR 0262](../decisions/0262-export-numeric-drafts.md)）。
  共通既定設定は明示操作で UserDefaults へ保存し、パッケージ・旧パス設定のない時だけ適用する（[ADR 0263](../decisions/0263-common-export-defaults.md)）。
  名前付き書出しプリセットは別の版付き UserDefaults に保持し、明示適用だけで現在の下書きを変える（[ADR 0264](../decisions/0264-named-export-presets.md)）。
- **文書の寿命**: 文書と同じ寿命の状態は `DocumentSession` に置き、開く・閉じるで丸ごと差し替える。
  非同期の処理は開始時のセッションを保持し、古い文書の処理が次の文書の状態へ届かない。
  文書への操作は省略できない `DocumentID` を受け、`ProjectController.session(for:)` だけが現行の文書と比べる
  （[ADR 0203](../decisions/0203-document-session.md)）。
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

#### 操作子・ブラウザ・ミックス

- **値の操作子**: ヘッダの音量・パン、クリップのハンドル、パラメーター欄は `ValueClicks` の同じ規則で押下を受ける。
  単クリックは値を変えず、ドラッグは押した値からの相対量、ダブルクリックは既定値の仮表示と間隔後の記録、3 回目は入力欄を開く。
  入力欄（`ValueEntryField`）は編集中のキーを先に受け、メニューの Space や文書の Undo へ渡さない
  （[ADR 0153](../decisions/0153-value-control-clicks-and-typed-entry.md)）。
  ヘッダの文書交換時は押下・リセット待ち・数値欄を取り消す。操作中は押下時のコールバックを保持し、新文書の試聴を触らない
  （[ADR 0193](../decisions/0193-header-mix-gesture-document-scope.md)）。
  クリップのゲイン・フェードも文書交換で未確定操作と数値欄を取り消し、押下時の編集先を保持する。
  投入済み数値入力は実行時に文書を再照合する（[ADR 0194](../decisions/0194-clip-mix-gesture-document-scope.md)）。
  ヘッダの横移動量ゼロは表示位置を経由せず元の Float を保ち、縦ドラッグと解放時の原点復帰を no-op にする。
  表示下限未満の正の音量を無音へ変えない（[ADR 0153 の後続修正](../decisions/0153-value-control-clicks-and-typed-entry.md)）。
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
- **アレンジの直後への複製**: 単一クリップの Command＋D はアレンジ自身のキー等価と `keyDown` から、既存の `ClipMove`・`clip.duplicate` へ渡す。
  現在の窓の終端と複製終端を整数で検査し、拍へ丸めず、複製側を選択する。
  入力欄・他ペイン・未確定ジェスチャー・文書交換中・キーの自動反復は対象外とする
  （[ADR 0188](../decisions/0188-arrange-adjacent-clip-duplication.md)）。
- **選択トラックのミュート・ソロ**: ヘッダとインスペクターは `arrangeMixActions` を共有し、操作時の文書・トラック ID 群を捕捉する。
  ヘッダの反転は待ち列で順序を保ち、編集の手番の現行値から決める。インスペクターの絶対値は no-op で履歴を変えない。
  `MixerSwitchBatch` が既存の `param.set` 配列を単一 Undo・保存へ渡し、選択外のソロを保つ。
  ピアノ行は単一対象を保つ（[ADR 0189](../decisions/0189-selected-track-mute-and-solo.md)）。
- **選択トラックの音量・パンの数値入力**: `MixActions.enter` をヘッダとインスペクターが共有し、`MixerValueBatch` が入力時の対象を保持する。
  手番の現行値から共通の音量倍率・パン差を求め、全対象の上下限で制限して既存の `param.set` 配列を 1 回送る。
  旧文書・欠けた対象は全体を拒否し、変更がなければ履歴を増やさない
  （[ADR 0191](../decisions/0191-selected-track-numeric-mix.md)）。
- **ヘッダの読み上げの音量・パン増減**: `MixActions.adjust` が操作時の対象と方向を捕捉し、同じ待ち列と `MixerValueBatch` を使う。
  現行値から 1 dB・5% の共通差分を計算し、単一対象の無音への段と複数対象の有限の dB 差を分ける。
  主スレッドの callback は同期捕捉し、別スレッドからは UI actor に渡す。読上げ値は確定値を使い、文書交換で要素を再生成する。
  （[ADR 0192](../decisions/0192-selected-track-accessibility-steps.md)）。
- **ヘッダの音量・パンの一括ドラッグとリセット**: 押下時の `MixActions.capture` が全対象と元の値を保持する。
  `MixerFaderBatch` が共通限界・試聴・確定前の全体照合を行い、`FaderPreview.members` を描画と音声へ共有する。
  取消と原点復帰は履歴を変えず、確定だけが既存のアクション配列へ進む
  （[ADR 0228](../decisions/0228-selected-track-live-mix.md)）。
- **P2 のアレンジ・ミックス編集**: 右ドラッグでクリップ選択、空白左ドラッグで編集区間選択、リージョン作成、相対音量・パンの一括操作に対応する
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

時間単位・ミキサーと MIDI の範囲は、値を返す製品 C ABI から `EchodeBridge.ModelLimits` へ渡す。
`Tick` に拍の入出力の丸めを集め、ピアノ専用の変換へ依存しない。
ループの UI 下書きは整数の `LoopRange` で共有し、未変更の拍入力欄は元の端点を保持する。
既存の全時刻フィールドの値型化は別の範囲に残す
（[ADR 0200](../decisions/0200-shared-model-units-and-limits.md)、[ADR 0201](../decisions/0201-integer-loop-drafts.md)）。

`echode-actions` の登録簿と公開スキーマから `EditAction: Encodable, Sendable` を生成する。
型付きの単一／一括 `Document.perform` は既存の文書ワーカーへ送り、送信前の符号化失敗・取消では編集しない。
生成物を通常検査で照合する。controller の編集はすべて生成型の配列を送り、`applyEdit` で順序・文書照合・履歴・入力記憶を共有する。
新規トラックへのドロップは `ReferencingEditAction` の結果参照で、同じ一括編集の前の結果を指す。raw JSON の互換入口だけを残す
（[ADR 0202](../decisions/0202-generated-swift-edit-actions.md)）。

編集 JSON は単一のアクション、または単一 Undo のアクション配列を受ける。
配列は同じ文書ワーカーの `DocumentHost::apply` で全体を確定・巻戻しする
（[ADR 0124](../decisions/0124-selected-track-fx-batches.md)）。
命令は制御スレッドへ送ってすぐ戻り、ロックを保持したまま Swift を呼ばない。
配列を借りる同期クロージャーの間はハンドルを保持し、ポインタを外へ出さず `await` しない。
古い保存版へ後退した復旧はスナップショットの `historyReport.recovery` で通知する。
Swift は文書公開と出力準備の前に確認し、取消なら元の文書を保持する
（[ADR 0183](../decisions/0183-recovery-consent.md)）。

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
  - **入力先と参照表示**: `ProjectController.pianoTrack` をアレンジ・デバイスの選択から分ける。
    `PianoTrackTarget` は文書とトラックを保持し、明示オープン・右側のネイティブ表・遅れた操作を同じ所有確認へ通す。
    アレンジのダブルクリックは再生開始位置を変えず、`PianoLocation` を共有ナビゲーションで一度だけ消費する。
    `PianoRenderInput.references` の可視ノートを編集ノートより先に描き、ヒット判定・ベロシティ・読み上げ対象へ混ぜない。
    表示・入力先は一時状態、Solo/Mute は通常の順序付きパラメーター編集とする（[ADR 0181](../decisions/0181-piano-reference-workflow-and-keys.md)）。
  - **共通の表示範囲**: `CanvasNavigationState.pianoCamera` が文書内の時間・音域・倍率を共有する。
    トラック切替とエディタの再生成で復元し、切替先の既存選択へ自動移動しない。
    現存トラックと文書の所有確認を通して書き戻し、別文書では初期化する。
    保存・Undo の対象にはしない（[ADR 0174](../decisions/0174-shared-piano-viewport.md)）。
  - **自動／固定グリッド**: `PianoCamera.gridMode` が自動または固定音価を保持し、現在の横倍率から実効 `grid` を計算する。
    ネイティブの `PianoGridBar` を上端へ置き、入力・ノート編集・矢印・ルーラーと縦線を同じ幅へそろえる。
    記憶長は丸め直さず、変更時は未確定の押下を取り消す。
    文書内の共有と所有確認は共通カメラに従い、保存・Undo の対象外とする（[ADR 0185](../decisions/0185-piano-auto-and-fixed-grid.md)）。
    固定音価は通常・三連符（2/3）・付点（3/2）の全音符〜128 分音符を整数 tick で持つ。
    自動の候補は通常音価のまま、音価名は基準の分母と種類から作り、tick の逆数を分母にしない
    （[ADR 0186](../decisions/0186-piano-triplet-and-dotted-grids.md)）。
    拍・4 拍の時間目印は吸着幅と独立した `PianoFrame.gridLineTicks` で保持する。
    曲の先頭からの位相と可視範囲だけの生成を保ち、細分線・拍・4 拍の順で同位置は一度だけ描く
    （[ADR 0187](../decisions/0187-piano-independent-time-guides.md)）。
  - **ルーラーとループ**: クリックで開始位置記憶、ドラッグでループ範囲指定（[ループ再生](../interaction-design.md#ループ再生)）。
    両エディタはクリックの解放時に開始位置を記憶し、ドラッグでは範囲だけを 1 回確定する。
    各グリッドへの吸着と Shift、逆向き・取消、有効／無効の帯と範囲欄の同期を持つ。
    アレンジは単一 Metal フレームの上端 24 pt を使い、行の入力と描画はその下へそろえる。
    既存端は上端のグリップで反対端を固定して調整する。交差を制限し、外部の範囲・オンオフ変更でも取り消す
    （[ADR 0158](../decisions/0158-ruler-loop-edge-handles.md)）。
  - **ノートの入力と記憶長**: シングルクリックで記憶長入力、新規ドラッグで長さ・音高変更。記憶長は編集セッション内で全トラック共有し、Undo で戻す
    （[ノートの入力](../interaction-design.md#ノートの入力)）
    クリックと空白ドラッグは接続済み。押下時の開始・クリック長を保持し、ドラッグでは終端と音高を仮表示して解放時に 1 回だけ追加する。
    Shift は吸着を外すが、新規ドラッグの最短長はグリッド幅とする
    （[ADR 0171](../decisions/0171-piano-blank-drag-insertion.md)）。
    選択・追加・単一ノートの長さ変更を全トラックで記憶し、追加・長さ変更の Undo / Redo で戻す。
    保存はせず再オープン時は現在のグリッド幅を使い、記憶後は入力時に丸め直さない
    （[ADR 0172](../decisions/0172-piano-click-and-input-length.md)）。
  - **移動・伸縮・右クリックメニュー**: 時間グリッド吸着（Shift で解除）、複数選択の相対移動と一括削除
    （[ノートの選択](../interaction-design.md#ノートの選択)）。
    単一ノートのドラッグは接続済み。Shift の吸着解除・最短 1 tick と、解放時の座標・修飾キーによる単一確定を使う
    （[ADR 0167](../decisions/0167-piano-note-drag-snapping.md)）。
    単一トラックの右ドラッグ枠選択・Command＋クリックの追加と解除・右クリック削除メニューを接続した。
    選択集合は controller が保持し、複数選択の単一ノート欄・入力記憶への誤反映を防ぐ。
    Delete は既存の削除をトランザクションへまとめ、リンクした同じノートを重複削除しない
    （[ADR 0176](../decisions/0176-piano-note-selection-and-deletion.md)）。
    選択済みノートの本体ドラッグは選択全体の相対移動へ接続し、`PianoMoveGesture` が押下時の全幾何とカメラを保持する。
    つかんだ開始の絶対吸着から共通差分を求め、時刻 0・最大終端・音高の共通上下限で制限する。
    `PianoMovePreview` の全対象を仮表示し、解放時の Shift と座標で再計算する。
    `PianoMoveChange.actions` が待機列の手番で全幾何を検証し、同じ内容のノートを一度だけ単一トランザクションへ送る。
    選択と両入力記憶を保ち、選択変更でも取り消す（[ADR 0178](../decisions/0178-piano-batch-note-movement.md)）。
    選択済みの端ドラッグは `PianoResizeGesture` の共通の端の差分で伸縮し、最短長・時刻 0・整数上端で全体を制限する。
    `PianoResizePreview` を仮表示し、`PianoResizeChange.actions` が全幾何を手番で検証・重複排除して単一トランザクションへ送る。
    反対端・音高・選択と両入力記憶を保つ（[ADR 0179](../decisions/0179-piano-batch-note-resizing.md)）。Command 複製移動も同じ幾何を使う。
    Command 押下では選択切替を保留し、クリックの解放と複製ドラッグを分ける。`PianoMoveGesture.copies` が新 ID を保持する。
    `PianoMoveChange.actions` は同じ元ノートを一度だけ `note.duplicate` へ送り、成功後だけ複製側を選択する。
    元と両入力記憶を保ち、同じ取消と共通境界を使う（[ADR 0184](../decisions/0184-piano-note-duplicate-drag.md)）。
    矢印は `PianoStep` の対象 ID と相対差分だけを保持する。`enqueuePianoStep` がイベント順を捕捉し、手番の現行幾何で全対象を検証する。
    共通境界とリンクの重複排除を通して 1 回のトランザクションにし、キーリピートも絶対幾何の上書きにしない
    （[ADR 0181](../decisions/0181-piano-reference-workflow-and-keys.md)）。
    ノートの時刻編集による固定窓の拡張は、描画側で窓を無視せずモデルへ反映する
    （[スマートクリップ](model-and-history.md#45-スマートクリップ)、[ADR 0168](../decisions/0168-note-edit-clip-growth.md)）。
  - **ベロシティ編集**: ノート上の Option＋上下ドラッグと下部ベロシティ欄の双方向接続、複数選択の同量増減
    （[ベロシティの編集](../interaction-design.md#ベロシティの編集)）
    単一ノートの Option ドラッグと共通入力値は接続した。幾何を保持する `note.velocity` と専用の仮表示を使い、解放時に単一確定する。
    選択・成功した調整の値を全トラックで記憶し、入力長とは別に Undo で戻さず、再オープン時に 100 へ初期化する。
    下部の棒も同じ仮表示・単一確定へ接続し、時刻内の全音高とアクセシビリティの値・増減を扱う
    （[ADR 0175](../decisions/0175-piano-velocity-lane.md)）。共通幾何を入力・描画・読み上げへ使い、ノートグリッドと境界を分ける。
    選択済みのノートと棒のドラッグは選択全体の同量増減とし、0／127 の共通制限で強弱差を保つ。
    押下時の全対象と保存値を手番で検証し、共有内容のノートを一度だけ単一トランザクションへ送る。
    選択と入力長を保ち、つかんだ音の変更後の値を次の入力へ記憶する。選択変更でも取り消す
    （[ADR 0177](../decisions/0177-piano-batch-note-velocity.md)）。
  - **トラックセレクターとドラム入力**: 右端パネルで「表示」「編集」「ソロ」「ミュート」を独立切替し、フォルダ型ドラムラック（§8.5）の親 MIDI を音色名行で入力する
    （[トラックセレクター](../interaction-design.md#トラックセレクター)）
- **MPE・アーティキュレーション・コード（後続 Phase）**: MPE マイクロレーン、Articulation Set、コード推定とコードトラックに対応する
