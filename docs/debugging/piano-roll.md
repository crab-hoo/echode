# デバッグ手順: ピアノロール

ピアノロールの選択・移動・伸縮・ベロシティ・吸着・キー入力と表示範囲の症状を切り分ける。
最初の診断と記録する項目は [デバッグ手順](../debugging.md) にある。症状の一覧は `--outline` で見る。

## 選択・キー入力・表示範囲

### ピアノの入力先・参照表示とアレンジ選択が一致しない

通常のアレンジ選択は入力先を変えない。右の「編集」かアレンジの空白・MIDI クリップのダブルクリックで明示的に開く。
`pianoTrack` と `selection`、操作時の `PianoTrackTarget.document`、公開スナップショットの現存トラックを分けて調べる。
参照は `PianoRenderInput.references` と同じフレームの一覧を照合し、ヒット判定・ベロシティ・読み上げに混ざっていないか確認する。
`piano-target-ui`・`piano-arrange-open-ui`・`piano-reference-ui`・`piano-track-selector-ui` で切り分ける。
ダブルクリック後の再生成で位置が戻る時は `pianoLocationID` の消費を見る。通常の切替では共有カメラを保つ。
`spike-piano-frame` の `checkedReferenceNotes` と `mismatches` は倍率 1・2 の参照色・編集優先とレーンへの非混入を含む。

### 矢印の連続入力で一部だけ動く・前の状態へ戻る

`enqueuePianoStep` のイベント順と `PianoStep` の相対差分、待機列の手番での全 ID 検証を確認する。
ドラッグとは異なり、押下時の絶対幾何を再使用しない。共通上下限とリンク中身の重複排除は同じである。
`piano-key-step-ui`・`piano-key-step-bounds-ui` で保存・履歴・境界と旧文書の拒否を確認する。

### Return が挿入する・一時停止しない・一覧や入力欄から再生へ漏れる

ピアノは Return とテンキー Enter をレスポンダーへ渡し、メニューの共通 Pause に送る。
サンプル・FX 一覧のフォーカス時は `performKeyEquivalent` が配置を先に受け、入力欄ではメニューを無効にする。
`piano-return-ui`・`browser-return-ui` と `plugin-key-events`・`plugin-key-socket`・`plugin-key-ui` で経路を分ける。
実画面のメニュー・IME・VoiceOver・キーリピートと聴感はユーザー確認に残す（[ADR 0181](../decisions/0181-piano-reference-workflow-and-keys.md)）。

### ピアノロールの枠選択・削除が想定と違う

右ドラッグはノートグリッド内から始める。下部の棒・鍵盤・ルーラーからは始めず、枠は発音窓内の可視ノートとの部分交差を使う。
`PianoGridView.selectionPress` が仮の枠（`PianoInteraction` の射影）、`ProjectController.noteSelection` が確定選択である。
複数選択の時の `noteFocus == nil` は正常で、単一ノート欄や記憶値へ任意の代表を流さないための区別である。
Delete は `deleteNotes` の待機列の手番で全 ID を検証する。リンク配置の二重削除と、古いメニュー・旧文書の拒否を分けて調べる。
`piano-note-selection-ui`・`piano-note-selection-cancel-ui`・`piano-note-selection-delete-ui`・`piano-note-linked-delete-ui` を使う。
選択済み本体は一括移動、端は一括伸縮する。未選択ノートの通常押下は単一選択へ戻る。
実画面の枠とメニュー、VoiceOver の操作感はユーザー確認とする（[ADR 0176](../decisions/0176-piano-note-selection-and-deletion.md)）。

### リンク配置が増えるとノート削除だけが遅くなる

`Project::delete_note` から `placement::delete` を調べる。指定トラックの同じ内容は一度だけ探索し、ノート列は探索用に複製しない。
最後のノートの削除時は `remove_auto` が対象の自動配置を持つ別トラックだけを複製し、固定リンクの窓と内容を残す。
`cargo test -q -p echode-model --lib model_scale_tests::delete` は、共有内容の成功・不正 ID・空の内容と別トラックの拒否を計数する。
自動配置の所属が現在・別トラック・なしの場合の複製回数、固定窓・元の版・変更セットの往復も区別する。
ノート探索回数が配置数との積になれば内容の重複排除、別トラックの複製が増えれば除去前の借用による確認を見る。
描画・保存・実機の時間はこの回帰で測らず、製品の操作時間全体の改善率と混同しない。

### トラック切替でピアノロールの表示範囲が変わる

文書内の共通カメラは `CanvasNavigationState.pianoCamera` で、トラック別の復元は行わない。
切替先の既存選択への自動表示と、切替後の Tab・編集・再生ヘッド追従による移動を区別する。
`piano-shared-viewport-ui` は切替・再生成・空トラック集合・旧文書の書戻し拒否と別文書の初期化を確認する。
別文書で位置が残る場合は `setNavigation` の文書 ID と `prepare` の初期化、書戻しの `permits` を見る
（[ADR 0174](../decisions/0174-shared-piano-viewport.md)）。

## 移動・複製・伸縮・ベロシティ

### Command ドラッグが元を動かす・選択が消える・強弱が変わる

押下時の `PianoMoveGesture.copies` と解放時の `PianoMoveChange` を確認する。Command は押下で保持し、Shift と位置は解放時を使う。
クリックだけなら解放時に選択切替、ドラッグなら端からでも複製し、成功後の `selectingNotes` で複製側を選ぶ。
`note.duplicate` は共有内容の元を複製するため、リンクの同一ノート ID は一度だけ送る。強弱を 7 bit の `note.insert` で作り直さない。
`note-duplicate-model`・`piano-note-duplicate-ui`・`piano-note-duplicate-targets-ui`・`piano-note-duplicate-cancel-ui` で切り分ける。
固定窓を広げた時の重なりは従来どおり全体を拒否する。実画面・VoiceOver・聴感はユーザー確認とする
（[ADR 0184](../decisions/0184-piano-note-duplicate-drag.md)）。

### 複数選択の移動で間隔が崩れる・一部だけ動く・確定されない

選択済みノートの本体を通常ドラッグする。3 pt 未満の微動とクリックでは編集せず、端と Command は別操作である。
`PianoMoveGesture.originals` が画面外も含む押下時の全対象、`PianoMovePreview` がモデルを変えない全対象の仮表示である。
つかんだ開始の絶対吸着から差分を求め、全対象の開始・終端・最低音・最高音で共通制限する。上下端で全体が止まるのは正常である。
解放時の座標・Shift と、`PianoMoveChange.actions` の手番での全幾何検証を分けて確認する。
`piano-note-move-batch-ui`・`piano-note-move-batch-targets-ui`・`piano-note-move-batch-cancel-ui` を使う。
クリップ同士の重なりはまだモデルが全体を拒否する。他の固定リンクのトリムは保つため、共有内容の変更と窓の拡張を混同しない。
重なり拒否の再現では配置を固定して分ける。隣接小節へ順に入力するだけでは自動配置が結合し、壁を持つ別配置にはならない。
入力長・入力ベロシティは変えない。実画面と VoiceOver はユーザー担当である（[ADR 0178](../decisions/0178-piano-batch-note-movement.md)）。

### 複数選択の伸縮で長さの差が変わる・一部だけ伸びる

選択済みノートの左右端を通常ドラッグする。3 pt の横移動までは確定せず、縦移動は音高を変えない。
`PianoResizeGesture` はつかんだ端の絶対吸着から共通差分を求め、画面外も含む最短長・最小開始・最大終端で制限する。
最短 1 tick は Shift の有無にかかわらず共通制限する。長さの比率を保つ処理ではない。
`PianoResizeChange.actions` の全幾何検証・リンクの重複排除と、固定窓の拡張・重なり拒否を分けて調べる。
`piano-note-resize-batch-ui`・`piano-note-resize-batch-targets-ui`・`piano-note-resize-batch-cancel-ui` を使う。
入力長と入力ベロシティを保ち、解放時に 1 回だけ記録する（[ADR 0179](../decisions/0179-piano-batch-note-resizing.md)）。

### 複数選択のベロシティが動かない・強弱差が変わる

選択済みのノートを Option ドラッグ、または選択済みの棒をドラッグする。未選択の押下は単一選択になる。
差分は選択全体の最小・最大値で制限し、どれかが 0／127 に達したら全体が止まる。両端の値を含む時は動かないのが正常である。
`PianoVelocityGesture.originals` は押下時の全対象、`PianoVelocityPreview.values` は共有ノート ID ごとの仮の値である。
`PianoVelocityChange.actions` は待機列の手番で存在と元の保存値を検証し、1 つでも古ければ全体を拒否する。
取消と古い対象の拒否、共通上下限、入力値だけの応答の世代を `piano-velocity-batch-ui`・`piano-velocity-batch-targets-ui`・`piano-velocity-batch-cancel-ui` で区別する。
入力長は変えず、入力ベロシティだけがつかんだ音の変更後の値になる（[ADR 0177](../decisions/0177-piano-batch-note-velocity.md)）。

### Option ドラッグで音高が動く・ベロシティが引き継がれない

押下時の `PianoVelocityGesture` と通常の `PianoGesture` を分ける。横移動・グリッド・Shift はベロシティへ影響しない。
`piano-note-velocity-ui` は解放位置と単一記録、`piano-note-velocity-cancel-ui` は取消後の遅れた解放を検査する。
`note-velocity-model` は共有内容と配置窓を確認する。0 は無音であり、音源なしの無音と区別する。
記憶は `ProjectController.noteVelocity` の 7 bit 値で、Undo で戻さず再オープン時に 100 へ戻すのが仕様である。
実際のノートの保存値は 16 bit なので、表示との比較には上位 7 bit を使う。
応答後だけ値が古くなる時は、記憶の `generation` と選択の順序を確認する
（[ADR 0173](../decisions/0173-piano-note-velocity.md)）。

### 下部の棒がノート追加になる・音域外で消える・値が食い違う

棒は `PianoLayout.velocity`、ノートは `grid` と入力・描画の境界を分ける。
`PianoFrame.velocityNotes` は時刻内の全音高を取り、開始が左端より前の継続音には棒を作らない。
頭と棒の矩形は `PianoVelocityLane` を描画・ヒット・アクセシビリティで共有する。
`piano-velocity-lane-ui` は実入力と履歴・保存、`piano-velocity-lane-frame-ui` は問い合わせ・仮表示・読み上げの値、
`piano-velocity-lane-cancel-ui` は取消後の解放と空白クリックを切り分ける。
`spike-piano-frame` の `checkedVelocityBars` と `mismatches` は倍率 1・2 の 0・64・127 の GPU 画素を含む。
実画面と聴感はユーザー確認とする（[ADR 0175](../decisions/0175-piano-velocity-lane.md)）。

## 追加・吸着・入力長

### ピアノロールの空白ドラッグで追加されない・開始位置が動く・取消後に追加される

`PianoInsertion` の押下時の開始・カメラと、解放時の終端・音高を区別する。
開始の吸着は押下時、終端の吸着は解放時の Shift を使い、最短長は新規ドラッグだけグリッド幅である。
単クリックも記憶長で追加し、ダブルクリックの 2 回目は重ねない。3 pt 未満の移動は押下時の値を保つ。
`piano-note-insert-ui` は実 View・製品ブリッジ・単一 Undo と保存、`piano-note-insert-cancel-ui` は下書きの破棄を切り分ける。
仮表示はモデル ID のない `PianoRenderInput.insertion` であり、確定ノートや VoiceOver 要素へ混ぜない。
古い描画・異なる配置での押下は受けず、文書・トラック・カメラの変更と終了後の解放も追加しない
（[ADR 0171](../decisions/0171-piano-blank-drag-insertion.md)）。

### 多数の自動配置を結合するとノート編集が遅くなる

`placement::merge` の除去 ID 集合と内容ごとの時刻差の索引、`retarget` の一括付替えを調べる。
`cargo test -q -p echode-model --lib model_scale_tests::merge` は、配置と内容のキー訪問・対象トラックの複製を計数する。
統合数と全配置数の積に増えれば付替えと参照の再探索、内容数との積に増えれば内容ごとの除去走査を疑う。
固定リンクの発音位置・窓とミックス値、別トラック経由の拡張の保持、正負の内容原点とオフセット超過の拒否も同じ回帰で区別する。
この計数は付替えと除去だけで、結合対象の探索・ノート整列・描画・保存・実機の操作時間を測らない。

### ノート追加が無関係な内容の増加で遅くなる

`Project::insert_note_with_id` は新しいノート ID を常に検証するが、クリップ・内容の候補 ID は新規配置を作る時だけ検証する。
`placement::insert` の既存窓・拡張・結合の経路で `new_clip_ids_available` を呼んでいないか確認する。
`cargo test -q -p echode-model --lib model_scale_tests::insert` は ID 探索の回数と訪問ノート数を測り、候補の衝突・未使用時の互換・変更セットの往復も検査する。
この回帰は保存・描画・実機の時間を測らず、残るノート ID の線形探索と配列の複製・参照集合の費用は別に調べる。

### ズームでノートの吸着幅が変わる・固定幅が変わる・線と吸着が一致しない

`PianoCamera.gridMode` と実効 `grid` を区別する。自動は横倍率で変わり、固定は音価を保つ。
細かい固定幅の線は 4 pt 未満で間引くが、吸着幅と記憶済み入力長は変えない。
`PianoFrame.gridTicks` は吸着由来の間引いた細分線、`gridLineTicks` は独立した拍・4 拍の目印を含む描画順の列である。
目印の位置へ吸着しないのは正常。粗い幅で目印が欠ける場合は `piano-time-guides-ui`、
GPU の実画素は `checkedIndependentGuides` と `mismatches` で分ける（[ADR 0187](../decisions/0187-piano-independent-time-guides.md)）。
`piano-auto-grid-ui`・`piano-fixed-grid-ui` で実入力・旧押下の取消・文書所有確認を分けて調べる。
三連符・付点は `piano-rhythmic-grid-ui` で候補・音価表示と共通幅を検査する。
8 分三連符は 235,200 tick、付点 8 分は 529,200 tick。表示名を tick の逆数で作ると誤った分母になる。
トラック切替と再生成でモードが戻る場合は共通カメラと `onGridChange` の操作子反映を確認する。
`spike-piano-frame` の `checkedGridLines` と `mismatches` は倍率 1・2 の通常・三連符・付点の細分線の GPU 画素を含む
（[ADR 0185](../decisions/0185-piano-auto-and-fixed-grid.md)）。実画面の密度と VoiceOver はユーザー確認とする。
`grid pixel mismatch` は倍率・グリッド種別・tick・採取 x・実際と期待の色を示す。採取は描画と同じ Float 座標の被覆ピクセル中心で行う。

### 入力長が別トラックへ伝わらない・Undo で変わらない・再読込後に変わる

`ProjectController.noteInput.length` が未指定なら現在のグリッド幅を使う。再オープンで未指定へ戻るのは正常である。
記憶後のクリックは短い長さも丸めず、ドラッグだけ新規のグリッド最短長を使う。
`piano-note-input-length-ui` で選択・伸縮・別トラック入力と Undo / Redo、`NoteInputHistoryTests` で履歴枠と応答順を区別する。
ノート以外の編集の記録も `recordEdit` を経由し、選択や背景保存では枠を増やしていないか確認する
（[ADR 0172](../decisions/0172-piano-click-and-input-length.md)）。

### ピアノロールで延ばしたノートがクリップ端で切れる・移動すると消える

ノート自体の `start` / `length` と配置の `start` / `length` / `offset` / `kind` を分けて確認する。
小節未満の移動・複製だけでは自動配置を固定にしない。手動トリムと旧ジャーナルの移動後は固定なので区別する。
`piano-note-clip-bounds-ui` は実 View・controller・ABI・確定フレームと Undo / Redo・保存を通す。
自動窓は `origin = start - offset` から数えた中身の小節に合わせる（[ADR 0170](../decisions/0170-content-relative-auto-bars.md)）。
移動後にタイムラインの小節へ戻る場合は `placement.rs::refit_one` と `check_invariants` の基準を照合する。
`clip-content-bars` は編集・挿入・削除と履歴・保存、`journal-clip-bar-basis` は印のない旧操作の固定化を切り分ける。
固定窓の時刻編集での拡張は `placement.rs::grow_edited_window`、自動窓の結合は `refit_auto` を見る。
左側だけずれる場合は、共有内容の結合による参照先更新より後に開始とオフセットを一緒に動かしているかを確認する。
他の固定リンクの窓を保つこと、重複する拡張を編集全体で拒否することは仕様である
（[ADR 0168](../decisions/0168-note-edit-clip-growth.md)）。実画面の見え方・聴感はユーザー確認とする。

### ピアノロールのノートが Shift でも吸着する・短くできない・離すと位置がずれる

`PianoGridView.notePreview` が各ドラッグと解放の Shift を `PianoGesture.preview` へ渡しているかを確認する。
最短長は通常グリッド幅、Shift では 1 tick。左右端の固定側と、移動の長さは変えない。
`piano-note-drag-ui` は実 View の入力・単一 Undo・保存を、`PianoDragTests` の境界回帰は幾何を切り分ける。
解放時だけ変わる場合は、最後のプレビューをそのまま確定せず、解放時の座標・修飾キーで計算し直しているかを見る
（[ADR 0167](../decisions/0167-piano-note-drag-snapping.md)）。実画面の操作感はユーザー確認とする。
