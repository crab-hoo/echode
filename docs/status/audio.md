# 音声・再生・トランスポートの制約

[現在の範囲と制約](../status.md) の領域別の正本。
実機等の確認待ちは [ユーザー確認の一覧](../user-checks.md) の UC-番号で示す。

## 音声・再生・トランスポート

- **HAL**: 実機の一覧復帰・過負荷通知は未確認（UC-87・88）。通知しない音切れは検出できない。
- **テンポ境界**: 原点前後の実機・聴感は UC-62（[ADR 0275](../decisions/0275-shared-integer-tempo-index.md)）。
- **停止と一時停止**: 停止は記憶した開始位置へ戻り、一時停止はその場に保つ（[ADR 0155](../decisions/0155-remembered-playback-start.md)）。
  Return は、入力欄とブラウザ一覧の確定・配置を優先する（[ADR 0181](../decisions/0181-piano-reference-workflow-and-keys.md)）。
  実画面の操作感と、停止で戻る時の聴感はユーザー確認待ち（UC-17）。
- **再生ヘッドの追従**: 追従は保存しない表示状態で、キーの割当はない。
  停止中のズームで範囲外になった再生ヘッドは、次の再生・停止・位置移動まで表示しない（[ADR 0159](../decisions/0159-playhead-follow.md)）。
  ページ送り・VoiceOver は未確認（UC-18）。
- **開いた時の出力準備**: 再生は準備中の FX を含む出力の完成を待つ。
  停止中の処理負荷は開いた時から生じる（[ADR 0147](../decisions/0147-output-prewarm-and-background-media.md)、[ADR 0333](../decisions/0333-install-diff-graph-off-worker.md)）。
  実機の待ちと操作感は UC-19・69。
- **ライブ停止**: 停止中も無音の入力でプラグインを処理し、余韻とプラグイン内の鍵盤の音を出力する（[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
  実機と第三者プラグインの聴感はユーザー確認待ち（UC-25）。
- **位置移動と準備の取消**: 位置移動・未発音の準備の取消とオフラインの停止では、以前の発音状態を消す（[ADR 0099](../decisions/0099-transport-plugin-state.md)）。
- **バス・ループ・ゲイン・メーター**: 暗黙のステレオ→モノ変換はエラーにし、モノの音源は等電力でステレオへ上げる（[ADR 0095](../decisions/0095-product-audio-buses-and-continuity.md)、UC-72・[ADR 0337](../decisions/0337-mono-instrument-on-stereo-source.md)）。
  Phase 3 のレンダリングキャッシュの生成本体、Ogg、プラグインのサムネイルは未実装（[ADR 0108](../decisions/0108-phase1-loop-readiness.md)）。
  クリップゲインは `0..=1_048_576`（音声クリップのみ）で、負の配置オフセットはファイル前の無音として扱う（[ADR 0109](../decisions/0109-placement-bounds-and-plugin-capture.md)）。
  トラックメーターはフェーダー後の左右最大ピークで、LUFS やトゥルーピークではない（[ADR 0107](../decisions/0107-mix-controls.md)）。
- **Sine Pad と音源の選択**: 音源のないトラックのノートは無音で、旧プロジェクトへ暗黙の音源を補わない。
  従来の既定音を使うには Sine Pad を挿す（[ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)）。
- **クリップゲイン**: 実機の操作感・聴感は未確認（UC-48、[ADR 0241](../decisions/0241-unscaled-placement-pcm.md)）。
- **規模・ミックスと履歴（FCA-05〜07・19）**: モデル ID の線形探索と履歴のメモリ予算は残る（[ADR 0274](../decisions/0274-reused-track-plans.md)）。
  ノートと配置の編集の 1 段は触れた run と中身の一覧を持つ。多くを変える編集はその分を持ち、総量の予算はない（[ADR 0353](../decisions/0353-shared-placement-runs.md)）。
  子は親の FX・音量・パンを通る（UC-47・49・73、[ADR 0338](../decisions/0338-folder-bus.md)）。
  センド・サイドチェイン・ドラム用の MIDI 配布は使えない。遅延の計画だけがある（[ADR 0390](../decisions/0390-routes-in-the-latency-graph.md)）。
  PDC の第三者の報告値と聴感は UC-51・74、後段ノートの応答とプリロール時の聴感は UC-96（[ADR 0383](../decisions/0383-pdc-stage-notes.md)）。
  再生ヘッドの実画面と聴感は UC-89。物理的な出力遅延と描画周期は補正しない（[ADR 0373](../decisions/0373-pdc-playhead-position.md)）。
- **長い素材**: 全体を復号して 1 コピーを常駐させ、逐次読込みはない（[ADR 0273](../decisions/0273-shared-source-clip-windows.md)）。
