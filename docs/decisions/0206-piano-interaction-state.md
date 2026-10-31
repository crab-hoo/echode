# 0206. ピアノロールの操作状態を 1 つの列挙型へまとめる

- 日付: 2026-10-31
- 状態: 採用
- 対象: [構造監査](../reviews/2026-10-30-structure-audit.md) SPG-08
- 関連: [ADR 0167](0167-piano-note-drag-snapping.md)、[ADR 0176](0176-piano-note-selection-and-deletion.md)、[ADR 0178](0178-piano-batch-note-movement.md)、[ADR 0179](0179-piano-batch-note-resizing.md)

## 背景と再現

開始時の HEAD は `e813452` で、[ADR 0205](0205-playback-options.md) の作業の上に重ねた。
`PianoGridView` は同時に 1 つしか起きない操作を、11 個の独立した Optional で持っていた。
`mouseDragged` と `mouseUp` は決まった順に全種類を調べるので、2 つが残ると順の早いほうが勝つ。
監査は読解に基づく指摘だったが、修正前のコードで次の 3 件を再現した。いずれも解放イベントが届かなかった後の押下である。

- **押下を無視して前の操作が続く**: ルーラーの押下の後にノートを押してドラッグし、離すと、ノートは編集されず開始位置やループの要求が出た。
- **別の操作に変わらない**: 選択済みの本体の押下の後に端を押すと、伸縮ではなく移動になった。
- **描画済みのプレビューが残る**: プレビュー中は次の押下が古いフレームとして無視され、前の操作のドラッグが続き、解放で確定した。

AppKit が解放を届けない具体的な製品の経路（メニューやドラッグのセッションが入力を奪う等）は確認していない。
再現はテストで押下を続けて送ったもので、実機での頻度は未確認である。

## 決定

- **`PianoInteraction`**: `idle`・`ruler`・`marquee`・`insertion`・`note`・`velocity`・`move`・`resize` の列挙型にする。
  押下とプレビューを同じ case に持たせ、2 つの操作が同時に進む状態と、押下のないプレビューを型で書けなくする。
- **押下は前の操作を終える**: `mouseDown` の最初に、進行中の操作を取り消す。確定はせず、履歴も送信も変えない。
  フレームが古くて新しい操作を始められない押下でも、前の操作は終わる。その押下自体は操作を始めない。
- **入口の形**: `mouseDragged` は `PianoInteraction.drag` へ委ね、`mouseUp` は解放時の値の `switch` だけにする。
  `drag` は純粋な値の遷移で、ビューなしで検査できる。
- **読取り**: `selectionPress`・`velocityPreview`・`movePreview`・`resizePreview`・`displayedInsertion` は列挙型の射影として残す。
  テストと描画入力はこれまでどおり読める。`PianoRenderInput` の各欄は変えない。
- **部分的な取消の維持**: 選択の変更は `velocity`・`move`・`resize` だけ（`endIfBoundToSelection`）、大きさの変更は `note` 以外（`endOnResize`）を終える。
  取消の範囲の違いは従来の挙動のまま保ち、今回の変更で揃えない。

## 変わる挙動

- 上の 3 件が直る。解放が正常に届く操作の結果と Undo は変えない。
- 鍵盤の列など、どの操作も始まらない場所の押下でも、解放の届かなかった操作は終わる。
- 確定済みの操作・保存・音には影響しない。

## 監査の案との違い

- 単一ノートの case 名は `single` ではなく、既存の型に合わせて `note` とした。枠選択は右ボタンを追うので、左ドラッグの対象から外す（`followsLeftDrag`）。

## 回帰と基盤の保守

- `PianoInteractionTests` の 3 件は修正前に失敗することを確認した。`anEdgePressAfterAnUnreleasedGroupMoveResizes`、
  `aPressDuringAnUnreleasedPreviewEndsItWithoutCommitting`、`aNotePressAfterAnUnreleasedRulerPressEditsTheNote` である。
- 同じ suite で、全 case の取消の範囲・移動と伸縮のしきい値・枠選択が左ドラッグで動かないことを確認する。
- `PianoRollView.swift` を指す必須契約の反例を、同じ誤りを入れる新しい形へ更新した。
  取消の反例は `cancelGesture` が対象の case だけ残す形、選択変更の反例は `setSnapshot` の取消を外す形にした。
  解放時に最後の描画値を確定する反例は `mouseUp` の各 case、Shift を無視する反例は `PianoGesture.notePreview`（`PianoInteraction.swift`）へ移した。
- 型が排他を保証するので、排他そのものの必須契約は足さない。押下の置換は通常の `swift test` で確認する。
- 実画面の操作感と VoiceOver はユーザー確認である。新しいアクション・保存形式・音の処理・ログはないので、関連する基盤の更新は不要である。
