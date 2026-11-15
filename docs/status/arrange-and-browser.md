# アレンジ・ブラウザ・ミックス・画面の制約

[現在の範囲と制約](../status.md) の領域別の正本。
実機等の確認待ちは [ユーザー確認の一覧](../user-checks.md) の UC-番号で示す。

## アレンジ・ブラウザ・ミックス・画面

- **MIDI クリップの音高表示**: 実画面と VoiceOver はユーザー確認待ち（UC-01、[ADR 0165](../decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
- **クリップの移動と複製（UIF-03）**: 移動先で重なる配置は置き換え（削除・切詰め・分割）になる（[ADR 0154](../decisions/0154-clip-move-and-duplicate.md)）。
  ⌘D はアレンジにフォーカスがある時だけ有効で、押し続けの自動反復はしない（[ADR 0188](../decisions/0188-arrange-adjacent-clip-duplication.md)）。
  重ねるモード・自動クロスフェード・複数クリップは未接続。実画面の操作感と VoiceOver はユーザー確認待ち（UC-05）。
- **クリップのミックス操作の文書交換**: 実画面の文書切替・VoiceOver はユーザー確認待ち（UC-14、[ADR 0194](../decisions/0194-clip-mix-gesture-document-scope.md)）。
- **音声のドロップ**: 複数ファイルの同時ドロップ・自動スクロール・配置予定の波形と、同じトラックでの配置の重複は未接続（[ADR 0145](../decisions/0145-sample-browser-file-drops.md)）。
  ドラッグの実操作・見え方・VoiceOver はユーザー確認待ち（UC-06）。
- **サンプルブラウザのクリック（UIF-05・09）**: クリックの遅れとフォーカスが直ったかはユーザー確認待ち（UC-15）。
  遅れの量は未計測である（[ADR 0143](../decisions/0143-browser-click-and-audition-timing.md)）。
- **試聴**: プラグインを使わない。過大出力で止まり、次の試聴で戻る（[ADR 0344](../decisions/0344-sample-audition-output-guard.md)、UC-78）。
  表示中は無音の試聴出力を保つ（[ADR 0152](../decisions/0152-browser-audition-controls.md)）。
  隣の行の先読みと、長いファイルの読込み時間の計測はしていない（[ADR 0163](../decisions/0163-sample-overview.md)）。
  2 つの IOProc の加算と機器切替時の挙動、聴感は実機でのユーザー確認待ち（UC-16、[ADR 0135](../decisions/0135-sample-audition.md)）。
  長い素材の読込み中の試聴と、表示中の試聴の操作もユーザー確認待ち（UC-41・42）。
  試聴のループ（UIF-10）は再現しておらず、原因は未確定である（[照合](../reviews/2026-10-21-ui-feedback-audit.md)）。
- **2026-10-25 の操作の報告**: サンプル一覧の Space が製品のどこで止まっていたかは再現していない（[照合](../reviews/2026-10-25-user-feedback.md)）。
  実画面と聴感での解消はユーザー確認待ち（UC-20）。
- **選択トラックのミックス（P2-06）**: ピアノロールの行の Solo / Mute ボタンは単一トラックのまま。選択外のソロは解除しない（[ADR 0189](../decisions/0189-selected-track-mute-and-solo.md)）。
  無音の基準からの音量比が定まらない一括入力は、理由を示して拒否する（[ADR 0191](../decisions/0191-selected-track-numeric-mix.md)）。
  一括の切替・読み上げの増減・文書切替・ドラッグとリセットの実画面・VoiceOver・聴感はユーザー確認待ち（UC-12、UC-38〜40、[ADR 0228](../decisions/0228-selected-track-live-mix.md)）。
- **値の操作子（UIF-04）**: 実画面の操作感・VoiceOver・第三者プラグインの変換はユーザー確認待ち（UC-13、[ADR 0153](../decisions/0153-value-control-clicks-and-typed-entry.md)）。
- **ペインのリサイズと描画（UIF-01、JNK-01・02）**: リサイズ中の見え方はユーザー確認待ち（UC-02、[ADR 0144](../decisions/0144-unscaled-canvas-resizing.md)）。
  上下のずれ・再生開始時のがくつき・VoiceOver の解消はユーザー確認待ち（UC-03、[ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
  ちらつきの有無はユーザー確認待ち（UC-04、[ADR 0150](../decisions/0150-stale-drawable-frame.md)）。
