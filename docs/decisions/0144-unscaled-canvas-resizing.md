# ADR 0144: ペインのリサイズで前のフレームを伸縮せず、指定ヘッダ幅を保つ

- 日付: 2026-10-22
- 状態: 採用（`contentsGravity` の上書きは [ADR 0149](0149-flipped-canvas-anchor-and-fixed-transport-chrome.md) で置き換えた）
- 範囲: UIF-01。アレンジ・ピアノロールの Metal レイヤー、描画サイズ、トラックヘッダ幅の表示記憶

## 背景と再現

[操作の要望の照合](../reviews/2026-10-21-ui-feedback-audit.md#uif-01-ペインのリサイズでトラック幅が動く)で、原因を 2 つに分けた。
前のフレームを新しい枠へ引き伸ばす配置と、狭い layout の制限値を指定ヘッダ幅へ書き戻す処理である。
[ADR 0098](0098-pane-render-boundaries.md) の境界で切る処理だけでは、境界内の伸縮は防げない。

製品 View をウィンドウなしで生成する `CanvasResizeTests` を修正前に実行し、両方の回帰が期待値で失敗した。
前者は `scaleAxesIndependently` / `resize` と描画サイズの遅延、後者は幅 0・狭い幅を往復した後の幅と表示記憶の消失を検出する。
これはレイヤーの設定と幾何の再現であり、実画面の伸縮はユーザーの観察に基づく。

## 決定

- `CanvasSurface` が両方の表示面を共通設定する。
  `NSView.layerContentsPlacement` と `CALayer.contentsGravity` を左上固定にし、前の絵を等倍のまま切る。
  `layerContentsRedrawPolicy = never` を維持し、AppKit に Metal とは別の描画を要求しない。
- 枠・bounds のサイズ変更と backing scale の変更を受ける主スレッドが、描画入力を渡す前に `contentsScale` と `drawableSize` を更新する。
  ピクセル数は端数を切り上げ、空の表示面だけは 1 × 1 にする。暗黙のアニメーションは無効にする。
- 描画スレッドはレイヤーのサイズを書き換えない。
  取得済み drawable の大きさが入力と違う場合は既知の背景色でクリアし、その入力の版を消費せず次のコールバックで再試行する。
  GPU 完了待ちを主スレッドへ移さず、専用描画スレッドと休止・再開の規則を保つ。
- `ArrangePaneView.headerWidth` は利用者が指定した幅とし、layout では変更しない。
  現在のペインに収まる幅だけを、入力用ヘッダ・分割線・Metal の描画入力へ共通に渡す。
  `CanvasNavigationState.headerWidth` にも指定幅を記憶し、狭い状態でビューを作り直しても失わない。
  利用者がヘッダをリサイズした時だけ指定幅を更新する。保存形式・音・Undo は変えない。

## 回帰と診断

- 必須契約 `canvas-resize-ui`: 両方の製品 View で、左上固定と子の layout 前の drawable サイズを確認する。
  ゼロ・拡大縮小・端数を含む。負の対照は引き伸ばす配置へ戻す。
- 必須契約 `header-width-ui`: 幅 0・狭い幅の往復、描画入力、文書内の記憶、ビュー再生成、初回の既定幅を確認する。
  負の対照は制限値の書戻しを復活させる。
- 単体回帰で倍率 1・2、端数の切上げ、古い幅・高さの drawable の拒否を確認する。
  既存の `pane-boundary` とオフスクリーン GPU 検査は引き続き別の境界を検査する。
- `ArrangeMeasurement.sizeMismatches` は、サイズの不一致でクリアした callback の数である。
  リサイズ中の一時的な増加は正常であり、安定後も増える場合は入力の倍率・サイズと drawable を照合する。

## 未確認

実画面のペイン境界をドラッグ中の見え方、倍率の違う画面への移動、VoiceOver の操作感はユーザー確認とする。
通常回帰はウィンドウ・機器・表示リンクを開かない。自動検査の合格を実画面の確認済みへ置き換えない。
