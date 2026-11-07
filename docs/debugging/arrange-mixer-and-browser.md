# デバッグ手順: アレンジ・ミキサー・ブラウザ・画面

アレンジ・トラックのヘッダとミックス値・FX の行・サンプルブラウザと試聴・ペインと描画の症状を切り分ける。
最初の診断と記録する項目は [デバッグ手順](../debugging.md) にある。症状の一覧は `--outline` で見る。

## アレンジ・クリップ・トラック

### 再生ヘッドに表示が追従しない・勝手に戻る

まずトランスポートの「再生ヘッドに追従」がオンかを見る。横方向が優勢なスクロールは正常にオフにする（[ADR 0159](../decisions/0159-playhead-follow.md)）。
オンでも動かない時は、ルーラー・ノート・クリップの押下や数値入力欄、音声ファイルのドラッグによる保留中かを確認する。
停止中のズームで範囲外へ出た再生ヘッドは、次の再生・停止・位置移動まで表示しないのが仕様である。
再生中に止まる場合は `PlayheadFollower.awake` と、`PlaybackFeed` の `playing`・`tick` が進んでいるかを分けて見る。
`playhead-follow-ui` は判定・保留・解除、`playhead-follow-state-ui` は文書ごとの値を切り分ける。

### アレンジのノートが帯のまま・音高が逆・VoiceOver と高さが違う

中身の `PitchRange` は表示中のノートでなく全ノートから求める。スクロールやトリムしたリンク配置で縮尺が変わるなら索引の範囲を確認する。
`arrange-note-pitch` は音域と 1 pt 下限、`arrange-note-pitch-ui` は製品の Metal と VoiceOver 用の局所矩形を切り分ける。
`ArrangeFrame.instance` でノートの高さから行区切りの 1 pt を引いていないか、`accessibilityRect(for:)` が最後に提出したフレームを使うかを見る。
問い合わせだけの時間は `spike-arrange-query`、実画面の提出・見え方は S1 とユーザー確認で区別する。
`make debug-report` は検査の `target/verification/spike-arrange-query.log` も収集する。

### 置いた音声の頭や終わりでクリック音がする・フェードのハンドルが押せない

取込みはフェード 0 で置く（[ADR 0161](../decisions/0161-placed-audio-without-fades.md)）。素材の端の不連続であり、再生の不具合ではない。
ハンドルはクリップの端から 4 pt 内側に表示する。押せない時は `ClipControlLayout.entering` / `leaving` とクリップの矩形を比べる。
以前のジャーナルの `audio.place` は 5 ms で再生する（必須契約 `journal-place-fades`）。

### 素材の終端後に波形が続く・テンポ変更で波形の位置がずれる

音の変化と波形だけの誤表示を区別し、`Waveforms::fill` の問い合わせ範囲・素材フレーム数・テンポ点を残す。
`audit_cur02_waveform_keeps_silence_after_the_source_ends` と
`audit_cur03_waveform_maps_each_column_through_the_tempo_map` が、列ごとの時間対応を通常回帰で検査する。
`waveform_partial_views_keep_multiple_tempo_changes_and_trim_offsets` で全幅・部分幅・キャッシュ再読込みを比較する。
EOF 超過と途中のテンポ変化は独立した条件である（[CUR-02 / 03](../reviews/2026-10-25-code-audit.md)）。

### FX が違うトラックへ入る・並びが変わらない

候補を開いた文書セッション、追加操作時の選択トラック、ドロップ先トラックを区別し、`device.reorder` / `device.copy` と状態ハッシュの引き継ぎを `fx-chain-ui` で確認する（[ADR 0118](../decisions/0118-track-fx-chain-editing.md)）。
インスペクターは `WorkspaceInspectorView` → `InspectorAddFX` → `openPlugins(on:)` の順で、表示した文書とトラックを保持する。
`fx-inspector-ui` は実ボタンから空のチェイン・対象の選択変更・旧文書を確認する。名前クリックによる独自 GUI の入口とは別である。

### トラックが違う位置へ追加される・追加後の選択が違う

`TrackInsertionRequest` の文書セッションと `after`、ジャーナルの `track.add` / `track.insert` と `into_folder` を区別する。
展開フォルダは最後の子、折りたたみ中は部分木の外側、末尾ボタンは最上位末尾へ追加する。
`TrackInsertionTests` と `track_insertion` のログで操作後の選択変更・消えた対象・旧文書の拒否を確認する
（[ADR 0127](../decisions/0127-positioned-track-insertion.md)）。

### 複製が違う位置に入る・二重に増える・元のノートまで変わる

`TrackDuplicationRequest` の文書セッションと `sources`（選択された祖先を持つ行は除く）、ジャーナルの `track.duplicate` の `source` と `seed` を区別する。
複製の ID は `seed` から導出した UUIDv8 で、元と同じ中身 ID の配置が複製先に残っていれば独立化の不具合である。
`TrackDuplicationTests` と履歴・CLI の `track_duplication` で、位置・選択・中身の独立・書出しの 2 倍一致を確認する
（[ADR 0129](../decisions/0129-track-duplication.md)）。

### クリップが違う位置・トラックへ移る・重なった配置が消える・複製の編集が元へ伝わる

`ClipMove` の文書・配置・トラック・開始・`copy` と、ジャーナルの `clip.move` / `clip.duplicate` の `seed` を区別する。
開始は押下時のポインターの拍からの移動量で決まり、四分音符へ吸着、Shift で解除、Command は離した時点の値を使う。
重なる配置の削除・切詰め・分割は置き換えの仕様で、分割の右側は `seed` から導出した ID を持つ。
複製の配置が元と同じ中身 ID なら独立化の不具合である。
`ClipMoveTests` と履歴・CLI の `clip_move` で、吸着・選択・置き換え・書出しの一致を確認する
（[ADR 0154](../decisions/0154-clip-move-and-duplicate.md)）。
単一クリップの ⌘D は `ClipDuplicationTests`・`ClipDuplicationBoundaryTests` で、現在の窓の終端・整数上端・文書・入力フォーカスを区別する。
文字入力中・別ペイン・未確定ジェスチャー・キーの押し続けで複製しないのは仕様である。
`ArrangeView.performKeyEquivalent` と `keyDown`、`ClipMoveTarget.adjacentStart`、送信後の `clipFocus` を確認する
（[ADR 0188](../decisions/0188-arrange-adjacent-clip-duplication.md)）。

### クリップ移動だけが無関係なノート数で遅くなる

移動と複製、配置の全置換・片側の切詰め・内側への着地による分割を区別する。
`clip_move::land` は分割の右片に ID が必要な時だけ予約を呼び、移動側はその時に `used_ids` を一度作る。
`cargo test -q -p echode-model --lib model_scale_tests::clip_move` で、分割なしの ID 集合収集が 0 回、分割ありで 1 回かを調べる。
テスト用のノート ID 訪問数と集合の作成回数で区別し、実時間の閾値にしない。
分割・複製の全 ID 収集、配置の探索・置換と未参照内容の除去、保存待ちは別の費用として残る。
`DuplicateId` で拒否される時は `split_piece_ids_cannot_reuse_any_stored_identity` と `seed` を確認し、再送で別 ID を暗黙に作らない。

### フォルダ開閉後に行・ミックス値・FX の対象がずれる

元スナップショットと表示用ハンドルの行数・ID、`TrackTreeInfo` と `row_map` を区別する。
ヘッダ、可視矩形、ミキサーとデバイスが同じ表示行から元トラックを参照することを
`FolderDisclosureTests` と `snapshot_ownership` の必須契約で確認する。
開閉後に古いフレームが反映される場合は、入力ハンドルと描画キャッシュの版も確認する
（[ADR 0128](../decisions/0128-folder-disclosure-and-row-projection.md)）。

## ペイン・描画・アプリ

### 開始位置の変更・ループ・メトロノームで、ボタンやルーラーが一瞬暗転する

トランスポートの欄・ルーラー・コマンドは `ProjectController.transportLocked`（文書なし、または `busy`）だけで無効になる。
`audioBusy` は応答待ちの有無であり、操作子の有効状態に使っていないかを確認する。
続けた操作が反映されない時は、`orderedAudioCommand` の順番待ちと、トグルが番の来た時の状態を読んでいるかを見る
（[ADR 0160](../decisions/0160-ordered-transport-requests.md)、必須契約 `transport-order-ui`）。

### 直したのに画面が変わらない

`doctor` でアプリが `stale` / `unknown` なら `make app` を実行し、`target/Echode.app` を開き直す。

### ペインのドラッグ中に絵が伸縮する・狭くした後にヘッダ幅が戻らない

`canvas-resize-ui` は両表示面の見た目の左上固定（反転した階層では AppKit が `.bottomLeft` を選ぶ）と即時の drawable サイズ、
`header-width-ui` は幅 0・狭い幅の往復と表示記憶、`transport-layout-ui` は状態行と再生ボタンの寸法の不変を確認する。
`CanvasSurface` がレイヤーのサイズを主スレッドで更新し、描画スレッドが後から古いサイズを書いていないか調べる。
`ArrangeMeasurement.sizeMismatches` は入力と異なるサイズの drawable に現在の入力を描いた数で、リサイズ中の一時的な増加は正常である。
安定後も増える場合は入力の幅・高さ・倍率と drawable のサイズを照合する。
大きさの違う drawable は入力と同じ画素位置で描き、入力の外は `CanvasBackground` の背景色に保つ。
行ラベルも入力の境界でクリップするため、下端の部分表示行が余白へ文字を出さない。
`drawable-mismatch-ui` は製品と同じ `CanvasRenderer.submit` を使い、画素・描画済みの版・休止状態を確認する。
不一致後に描き直されない時は、版を消費する条件と `publishLinkState` へ渡す不一致の状態を調べる。
描画部品だけのオフスクリーン比較を、この制御経路の確認の代わりにしない。
リサイズ中に暗い面や透明な余白が見える場合は、描画スレッドが古い大きさの入力で描いていないか、
`CAMetalLayer.backgroundColor` が背景色と一致しているかを確認する（[ADR 0150](../decisions/0150-stale-drawable-frame.md)）。
`ArrangePaneView.headerWidth` と `CanvasNavigationState.headerWidth` は指定幅、描画入力の `header.width` は現在の制限値である。
自動回帰は設定と幾何の確認であり、ドラッグ中の見え方は実画面のユーザー確認とする（[ADR 0144](../decisions/0144-unscaled-canvas-resizing.md)）。
絵が上下にずれる場合は、表示面の `layer.contentsAreFlipped()` と `contentsGravity` を照合する。
反転した階層では CA の `top` が見た目の下を指し、AppKit は見た目の左上を `bottomLeft` に変換する。
`contentsGravity` を `.topLeft` で上書きすると見た目の左下へ固定される（[ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
再生開始時だけずれる場合は、状態行などペイン外の高さの変化を疑う。状態行の表示器は準備中だけ見せ、場所は常に確保する
（[GUI のがくつきの監査](../reviews/2026-10-22-gui-jank-audit.md)）。
高さを戻しても縦位置が戻らない場合は、`ArrangePaneView.viewport`（指定値）と
`timeline.renderInput.camera.originTrack`（表示の制限値）を比べる。指定値が表示の制限で書き換わっていないかを見る。
横スクロール・横ズーム・表示中のトラック選択で指定値が変わらないこと、上スクロールと縦ズームが表示中の行から始まることも見る
（[ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
ピアノロールの表示やリサイズが重い場合は、`accessibilityBuilds` が描画のたびに増えていないかを見る。
要素は問い合わせ時にだけ作り、同じ描画版では再利用する。再利用時も画面座標と選択値は更新される（同 ADR）。

### プレイヘッド・リフレッシュレート・ペイン境界・行境界

プレイヘッドは音声が最後に書いた tick を補間せず描画する。`displayFrameRateRange`（最大 120 Hz）、ペイン縮小時の `clipsToBounds`（[ADR 0098](../decisions/0098-pane-render-boundaries.md)）、オフスクリーン検査の左余白比較点（`x=2 pt`）を確認する（[ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)）。

### キー入力・ホイール・検索・ループ欄

ヘッダの `keyDown` から上位への Space 転送、マウスホイールの行単位とトラックパッドのポイント単位（[ADR 0050](../decisions/0050-arrange-viewport.md)）、ピアノロールのドラッグ中のみの Escape と連続矢印キー（[ADR 0052](../decisions/0052-piano-keyboard.md)）、空選択時の検索解除（[ADR 0053](../decisions/0053-workspace-ui-foundation.md)）、ループ欄の確定時送信（[ADR 0055](../decisions/0055-loop-metronome.md)）を確認する。

## FX の行・デバイス

### FX 候補のダブルクリックで追加されない・一覧が重い

一覧は `PluginListTable` で、`mouseDown` の `clickCount == 2` が行のどこでも追加する。
追加されない時は、入力先トラック（`capturePluginInsertion` が nil）と文書の交換中（`busy`）を区別する。再走査中は追加を止めない
（[ADR 0162](../decisions/0162-plugin-candidate-table.md)、必須契約 `plugin-list-ui`）。

### 下部を開くと別のエディタが消える・高さが戻らない

⌥⌘3・4 は各エディタ、⌥⌘5 はアレンジ非表示の独立した切替である。
`workspace.layout.v2` の `editor`・`devices`・`editorsMaximized` と `editorHeight` を分けて確認する。
v1 の切替は保存されていないため、移行時にデバイスの開閉を推測しない。
壊れた v2 は既定値へ戻すので、古い v1 の削除を初手の対処にしない。
`workspace-layout-ui` は共通コマンドと設定復元、`workspace-stack-ui` は画面なしの製品の枠・順序を検査する。
両方開いた通常表示の最小高は 870 pt、最大化は 645 pt で、最小高の変化を高さ設定の喪失と混同しない
（[ADR 0146](../decisions/0146-independent-editor-stack.md)）。

### ヘッダの名前が出ない・名前で候補が開く・違う窓が開く

名前は行高 52 pt 以上かつ幅と字下げに余裕がある時に表示する。先頭以外は「…」のメニューで選ぶ。
`TrackFXNamesLayout` の矩形とカタログ名、スナップショットのチェイン順を確認する。
名前は `onOpenPlugin`、追加ボタンは `onOpenFX` であり、`fx-header-ui` で入力と古い対象の拒否を区別する。
その後のヘルパーの表示失敗は既存の `plugin-window-helper` の経路を調べる（[ADR 0139](../decisions/0139-track-header-fx-names.md)）。

### デバイスの一覧が隠れる・折りたたみ後の FX ドロップ先がずれる

名前の左の開閉状態を確認する。折りたたみは音やオフライン状態を変えず、検索・一覧・状態の本文だけを隠す。
展開しても一覧が戻らない時はパラメーターの取得失敗と区別する。
`device-disclosure-ui` で可変幅の列の中央前後と送り先 ID を確認し、配置・挿入先に固定の列幅を使っていないか調べる。
トラック切替・エディタ再生成で開閉が戻るなら `collapsedDevices`、文書交換後も残るなら、書込み先が `controller.session` と同じセッションかを確認する。
見えない検索欄がキーを消費する時は折りたたみ時のフォーカス解除を確認する（[ADR 0137](../decisions/0137-device-disclosure.md)）。

### 値の操作子のクリックで値が跳ぶ・ダブルクリックが記録されない・入力欄が閉じない

規則は `ValueClicks` の段階（押下・ドラッグ・既定値の表示中・3 回目待ち）で決まる。単クリックで値が変わるなら、
NSSlider の追跡や絶対位置の計算が戻っていないかを `track-mix-ui`・`fx-params-clicks-ui` で確認する。
既定値が記録されないなら、待ち時間（`NSEvent.doubleClickInterval`）の前に文書が変わって下書きが消えたか、
プレビューの開始（`beginFader`・`beginParameter`）が拒否されたかを区別する。3 回目で既定値が残るなら `mix-clicks-ui` を使う。
別の操作子へ移ると拒否される場合は、待ち対象と次の押下対象、先行プレビューの終了を区別する。
`if case .waiting(target)` は既存の対象値を照合している。全待ち状態へ広げて旧押下を流用しない。
`mixer-reset-target-ui`・`clip-reset-target-ui` は対象切替と個別の履歴・保存を検査するが、編集完了前の連続入力や実機のクリック間隔の保証ではない。
入力欄が赤いまま閉じないのは、範囲外・単位違い・プラグインの拒否（`converts` が真で `value` が空）である。
変換を持たないプラグイン（`converts` が偽）は数値だけを受ける。`helper did not read the parameter text` は通信の失敗で、
`plugin-text-parse` で実ヘルパーの `text_to_value` を切り分ける。
入力中の Space で再生が始まる時は、欄の `performKeyEquivalent` が先に受けているか、欄が最初のレスポンダーかを確認する
（[ADR 0153](../decisions/0153-value-control-clicks-and-typed-entry.md)）。

### パラメーター・バイパス・オフライン操作で全ボタンが一瞬暗転する

`ProjectController.busy` の完了時の値だけでは短い暗転を見落とす。`device-edit-enabled-ui` で操作中の全遷移を観測する。
`EditTurn.busy` は編集待ち行列の状態、画面の `busy` は作成・交換・終了の明示的な排他だけである。
排他の既定値を通常編集へ広げていないか、各ビューが個別の `historyBusy` と全体の `busy` を取り違えていないか確認する。
FX 追加での素材・保存済み状態の読込みや、同一ヘルパーの状態復元を待つ経路とは別である（[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。

### FX の一括追加が一部だけ・Undo が複数回になる

単一フォーカスと `selectedTracks`、クリック時の `PluginInsertionRequest` の文書・対象配列を区別する。
製品 ABI が配列を同じ `DocumentHost::apply` へ渡すことを `TransactionTests` と `fx-batch-ui` のログで確認する。
選択変更後の付け替え、旧文書、欠けた対象、途中失敗時の部分追加を分けて調べる（[ADR 0124](../decisions/0124-selected-track-fx-batches.md)）。

## サンプルブラウザ・試聴・ドロップ

### サンプル検索で見つからない・配置先や位置が違う

登録フォルダの一覧と索引の読込中表示、読込失敗の詳細を確認し、素材変更後は更新ボタンで再読込する。
索引は隠しファイル・シンボリックリンク・パッケージ内部を含めず、重なる登録から同じパスを重複表示しない。
配置は `AudioImportRequest` の文書セッション・トラック・tick を使う。
定期更新の `audio.tick` とライブの `playback.position.tick` を区別し、非同期待機後の選択へ付け替えない。
`SampleBrowserTests` と必須契約の検索・配置ログを確認する（[ADR 0122](../decisions/0122-sample-library-search-and-placement.md)）。

### 音声をドロップできない・別の行や位置へ入る・トラックだけ残る

対応音声のファイル URL 1 つと copy 操作だけを受ける。MIDI・複数 URL・ヘッダ・分割線・領域外は拒否する。
`sample-browser-content-ui` は表示と URL の書出し、`sample-drop-entry-ui` は AppKit の入口と文書交換を切り分ける。
`sample-drop-ui` で現在のヘッダ幅・カメラ・折りたたみ後の行・Shift を確認する。ダブルクリックの再生位置配置とは別の経路である。
受理後の失敗は文書のエラーを確認し、復号・消失した対象・同トラックの配置重複を区別する。
トラックだけが残る場合は `audio-import-transaction` と `DocumentHost::apply` の失敗した要素の番号を調べる。
PCM と索引の残存は従来の仕様で、モデルの部分更新とは別である（[ADR 0145](../decisions/0145-sample-browser-file-drops.md)）。

### サンプルの試聴が鳴らない・古い素材が鳴る・止まらない

スナップショットの音声状態 `audition` の `request`・`handled`・`phase`・`error`・`sample_rate` を見る。
`handled` が `request` に届かなければ試聴スレッドが処理前、`failed` は復号か出力準備の失敗、`sample_rate` が空なら出力は閉じている。
機器の通知を受けると出力を閉じ、`stopped` と機器変更のエラーを残す。
ボイスは `echode-engine` の試聴の単体回帰、スレッドとコールバックは必須契約 `sample-audition` で切り分ける
（[ADR 0135](../decisions/0135-sample-audition.md)）。

### 試聴が遅れる・意図しない試聴が鳴る・クリックでフォーカスが移らない

`make debug-report` の `app-logs/audition.ndjson`（アプリが書く要求ごとの記録、直近はスナップショットの `audition.history`）を読む。
`input_delay_us` は入力イベントから要求まで（画面の待ち）、`picked_us` から `handed_us` は試聴スレッドの各段、
`heard_us` はその音を採用したブロックの出力時刻である。`opened_output` が真なら出力を開き直している。
`input` が `other` の要求、同じパスの繰り返し、`superseded` の多さで、利用者の操作以外の要求や連続した選択を区別する。
`first_decoded_us` と `converted_us` は最初の塊、`decoded_us` は全体の復号であり、長い素材では受渡しより後になる。
`waveform.loaded_frames` が止まり先読みが埋まっている間は正常な待ちである。
初回の読込み中に出力が閉じた場合は、機器変更・失敗とアイドル終了を区別する。
`sample-audition-loading` は読込み中のアイドル終了を拒否し、停止後の閉鎖も確認する。
`starved_frames` の増加は読込みが再生に追い付かなかった出力フレーム数で、素材の無音や EOF とは異なる。
必須契約 `sample-audition-stream` と機器なしの `spike-audition` で先頭の受渡しを切り分ける。
波形のパスが選択と違う場合は `SamplePreviewView` のパス照合、波形の末尾は `complete` と元の `frames_per_peak` を調べる
（[ADR 0151](../decisions/0151-streaming-sample-audition.md)）。
クリックの選択とフォーカスは必須契約 `sample-browser-click-ui` で確認する。
`settings.visible` が真なら無音でも出力を保持し、`output_error` は素材の要求前の準備失敗を示す。
`settings.gain` が 0 なら試聴だけが無音になる。自動試聴の設定は `workspace.audition.v1` を見る。
履歴の `start_frame` は元サンプル位置、波形の `first_frame` と `loaded_frames` は読んだ範囲の先頭と末尾である。
先頭以外からの `complete`・`decoded_us` は EOF までの読込み完了であり、未読の先頭部分の波形は含まない。

### ブラウザの波形が出ない・停止で消える・古い形のまま

一覧の下の波形は試聴の報告ではなく `SampleOverviews` の全体の読込みを先に使う（[ADR 0163](../decisions/0163-sample-overview.md)）。
選択の変化で `FileBrowserState.overviewTask` が始まったか、項目が `loading` / `ready` / `failed` のどれかを確認する。
`loading` のまま戻らない時は、より新しい要求で `cancelled` になった読込みを同じファイルの選択で読み直しているかを見る。
取消はブラウザごとのチャンネル（`SampleOverviews.channel`）の中だけで起こる。別のブラウザやテストが止めている場合はチャンネルの共有を疑う。
更新したファイルが古い形のままなら、サイズと更新日時（`FileStamp`）が変わったかを確かめる。`URL.resourceValues` の値は URL ごとに残るため使わない。
Rust 側は必須契約 `sample-overview`、表示と保持は `sample-overview-ui` で切り分ける。

### サンプル一覧にフォーカスがある時に Space で再生しない

一覧の `FileListTable.keyDown` が `onPlayback`（ワークスペースの「再生／停止」）を呼ぶかを、必須契約 `sample-list-space-ui` で確認する。
呼んでも鳴らない時は `WorkspaceSession.enabled(.playback)`（シート表示中・文書交換中）と出力の状態を見る。
メニュー側の `workspaceTextEntry` が一覧のフォーカスで残るかは未確認である（[照合](../reviews/2026-10-25-user-feedback.md#fb-02-サンプル一覧での-space)）。
表示保持・音量・位置と実操作子は必須契約 `sample-audition-resident`・`sample-audition-gain`・
`sample-audition-position`・`sample-audition-controls-ui` で切り分ける（[ADR 0152](../decisions/0152-browser-audition-controls.md)）。
ブラウザ自身の選択の付け替えは要求を出さない（[ADR 0143](../decisions/0143-browser-click-and-audition-timing.md)）。

## ヘッダ・ミックス値

### 選択トラックのミュート・ソロが部分適用される／連打で戻らない

`MixerSwitchBatch` の文書・捕捉 ID 群・anchor・反転／絶対値を、操作後に変わった現在の選択と区別する。
`mixerSwitchTail` と `EditTurn` の手番で反転元の現行値を確認し、`param.set` 配列を 1 回送っていることを追う。
欠けた対象と旧文書は全体を拒否し、同じ絶対値は Redo を消さない。ピアノ右側の行ボタンは単一対象を保つ。
切分けは `MixerSwitch*Tests` と必須契約 `mixer-switch-batch-ui`・`mixer-switch-order-ui`
（[ADR 0189](../decisions/0189-selected-track-mute-and-solo.md)）。

### 音量・パンの数値入力で選択全体が変わらない／入力値まで動かない

`MixActions.enter` と `MixerValueBatch` の捕捉 ID 群・基準 ID・要求値を確認する。
数値入力・読み上げ増減に加え、ヘッダのドラッグ・既定値リセットも相対一括変更である。
`mixerSwitchTail` の手番で現行値を読み、最大音量 4・パン ±1 を最初に踏む対象で共通差分を止める。
無音の基準からの音量増加は比が決まらないため理由を表示し、他の無音は有限倍率でも無音を保つ。
古い文書・欠けた ID の全体拒否と no-op による Redo の保持を、`MixerValue*Tests` と
`mixer-value-batch-ui`・`mixer-value-bounds-ui` で切り分ける（[ADR 0191](../decisions/0191-selected-track-numeric-mix.md)）。

### 一括ドラッグでヘッダと音が一致しない／一部だけ確定する

`MixerFaderBatch` の押下時の集合・元の値と、`FaderPreview.members`・送信中／最新の集合を比較する。
音声は `audio.preview_faders` で完全な集合を受ける。出力準備中は `Basis` と現在の集合の差を導入時に反映する。
別ノブの押下で先行する試聴を確定する時は、押下時の表示値と新しいドラッグ開始時の原点を区別する。
確定が拒否された時は旧文書・削除／変更済みのメンバーを調べ、音が戻る時は確定計画の版と解除要求を照合する。
`MixerDrag*Tests` と `mixer-group-audio` で、共通限界・取消・原点復帰・ブロック長と RT を切り分ける
（[ADR 0228](../decisions/0228-selected-track-live-mix.md)）。

### ヘッダの読み上げ増減で選択全体が動かない・段数が違う・読み上げ値が先走る

`MixActions.adjust` と `captureMixerStep` の捕捉 ID 群・方向（±1）、`mixerSwitchTail` の手番の現行値を確認する。
AppKit の主スレッドの呼出しは同期捕捉、別スレッドからは UI actor に渡す。木の更新前の絶対値を繰り返し送らない。
共通限界で止まる値を `MixerStepBoundaryTests` で読み戻し、未確定の予測値を読上げ値に使っていないかを見る。
複数対象の無音からの増加拒否と、有限差分では −60 dB 未満を保つ規則を、単一スライダーの無音への段と区別する。
再オープン後の誤編集は `documentID` 変更時の `controlElements` の破棄と旧要素の文書照合を追う。
切分けは `mixer-step-ui`・`mixer-step-bounds-ui` と `MixerStepOrderTests`
（[ADR 0192](../decisions/0192-selected-track-accessibility-steps.md)）。

### 文書を開き直した後に古いヘッダ操作で値が変わる／数値欄が残る

`ArrangeHeaderView.documentID` の変更で `ValueClicks.cancel`・押下の破棄・欄の取消が走るかを確認する。
`mixActions` の更新は文書 ID より先に来るため、終了は押下時の `gestureActions` を使い、新文書の別の試聴へ送らない。
同文書の更新では押下を維持する。`mixer-gesture-document-ui`・`mixer-reset-document-ui` と
`MixerGestureDocumentTests` で保存・履歴・callback の所有者を区別する（[ADR 0193](../decisions/0193-header-mix-gesture-document-scope.md)）。

### 文書切替後に旧クリップのゲイン・フェード入力が反映される

`ArrangeView.audioDocument` の変更時に `cancelClipControls` が押下・下書き・タイマー・数値欄を取り消すかを確認する。
`setSnapshot` の下書き破棄だけでは旧押下と欄が残る。同じ配置 ID が再利用される再オープンでも文書 ID は異なる。
開始時の `ClipPress.edit`・操作中の `gestureEdit` と現在の `onClipEdit` を区別し、旧操作へ最新の編集先を貸さない。
投入済み数値入力は欄を閉じるだけでは止まらないため、実行時の文書照合も確認する。
`clip-gesture-document-ui`・`clip-gesture-callback-ui` は実入力・製品 ABI・履歴と保存再読込を検査する
（[ADR 0194](../decisions/0194-clip-mix-gesture-document-scope.md)）。

### 縦だけのヘッダドラッグや原点での解放で音量が無音になる

押下時の音量と横移動量を確認する。`MixScale.position` は −60 dB 未満を位置 0 へ切り詰めるため、
横移動ゼロでは表示スケールを往復せず元の Float を返す。`mixer-drag-identity-ui` で試聴・Undo / Redo・再読込を確認する。
非ゼロ移動の下限到達と区別する（[ADR 0153 の後続修正](../decisions/0153-value-control-clicks-and-typed-entry.md)）。

### 画面のミックス値・メーターと音が一致しない

確定スナップショットと一時的な `faderPreview`、`requested_plan` / `applied_plan` を確認する。
メーターはフェーダー後の左右最大ピーク（停止中は 24 dB/s 減衰、−60 dB 以下で 0、無音表示は `−∞`）。VoiceOver の増減段数や数値欄の `−` 解釈は `MixControlTests` で確認する（[ADR 0107](../decisions/0107-mix-controls.md)）。
