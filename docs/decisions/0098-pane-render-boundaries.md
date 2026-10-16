# ADR 0098: ペイン変更中も表示面を境界内へ収める

- 日付: 2026-10-15
- 状態: 採用
- 範囲: アレンジとピアノロールの表示面、SwiftUI の領域、Metal の未描画フレーム

## 背景

エディタや右ペインの開閉時に画面が大きく覆われる報告があった。親の縮小後、子をレイアウトする前の順序を製品 View で再現すると、子の `visibleRect` が親の幅380・高さ180を越えた。安定後の幾何や画面だけを見る検査では、この中間状態を確認できなかった。

macOS 14 以降は `NSView.clipsToBounds` の既定が false である。[Apple の説明](https://developer.apple.com/documentation/appkit/nsview/clipstobounds) でも、親を縮小して子を隠すレイアウトへの影響が示されている。

## 決定

`ArrangePaneView`、`ArrangeView`、`PianoRollView`、`PianoGridView` は明示的に境界で切る。SwiftUI のアレンジ領域とエディタ領域も、サイズ指定後に切る。子の次のレイアウトや GPU 完了を待って境界を直す方式にはしない。

描く入力がない場合も drawable の提出は続けるが、プールから来た未描画のテクスチャを「前の絵」として提出しない。既知の背景色でクリアして提出する。通常の描画・休止の条件、性能基準、GPU とメインスレッドの分離は維持する。

## 検証

通常の `CanvasBoundaryTests.contractShrinkingPanesClipThePreviousSurfaceBeforeChildLayout` は、親だけ縮んだ時点でアレンジとピアノロールの可視範囲を確認する。修正前は両方の幅・高さで失敗し、明示的な切取り後は通る。必須契約 `pane-boundary` は切取りを外したコピーも拒否する。これは製品 View の中間状態の検査であり、全 OS・全画面の視覚的な無瞬断を証明するものではない。実ウィンドウと Computer Use の結果は別に記録する。
