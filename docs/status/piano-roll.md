# ピアノロールの制約

[現在の範囲と制約](../status.md) の領域別の正本。
実機等の確認待ちは [ユーザー確認の一覧](../user-checks.md) の UC-番号で示す。

## ピアノロール

- **入力長**: 記憶した値は全トラック共有で、再オープン時に初期化される（[ADR 0172](../decisions/0172-piano-click-and-input-length.md)）。
  実画面の操作感はユーザー確認待ち（UC-07）。
- **複数ノートのベロシティ**: 一括調整の実画面・VoiceOver・聴感はユーザー確認待ち（UC-08、[ADR 0177](../decisions/0177-piano-batch-note-velocity.md)）。
- **表示範囲の共有**: 実画面と VoiceOver はユーザー確認待ち（UC-09、[ADR 0174](../decisions/0174-shared-piano-viewport.md)）。
- **ノートの編集**: 吸着・追加・ベロシティ・枠選択と、選択・移動・複製・グリッドの実画面・VoiceOver・聴感はユーザー確認待ち（UC-10、UC-32〜37）。
- **操作の状態の一本化（SPG-08）**: 解放が届かない製品の経路は未確認で、実画面はユーザー確認待ち（UC-11、[ADR 0206](../decisions/0206-piano-interaction-state.md)）。
- **クリップの大きさ**: 旧ジャーナルの移動・複製は、従来の固定化の規則で再生する（[ADR 0170](../decisions/0170-content-relative-auto-bars.md)）。
  ノートの所属替えと、隣のクリップで止める処理は未接続で、草案に残る（[ADR 0169](../decisions/0169-clip-growth-follows-notes-and-stops-at-walls.md)）。
