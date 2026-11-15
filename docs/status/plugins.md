# プラグイン・デバイスビューの制約

[現在の範囲と制約](../status.md) の領域別の正本。
実機等の確認待ちは [ユーザー確認の一覧](../user-checks.md) の UC-番号で示す。

## プラグイン・デバイスビュー

- **CLAP の互換性**: 第三者のテンポ同期・聴感は UC-85、ほかの互換性は UC-80〜83。
  遅延のある前段と後段・フォルダ FX のテンポ同期の聴感は UC-91（[ADR 0378](../decisions/0378-pdc-stage-transport.md)）。
  音声ポートのリスト・チャンネル数の動的変更は未対応。再有効化前の変更検出時はヘルパーを停止する（[ADR 0374](../decisions/0374-dimension-preserving-audio-port-rescans.md)）。
  MIDI 1.0 だけを受ける音源には MIDI 1.0 でノートを送る。同じ音高の重なりは最初のノートオフで止まり、MPE・MIDI 2.0 だけを受けるポートは鳴らない。
  第三者の聴感は UC-90（[ADR 0375](../decisions/0375-midi-1-notes-for-midi-only-clap-ports.md)）。
- **遅いプラグインの走査**: 5 秒で答えないバンドルは後で 120 秒待ち、なお答えなければ次の起動まで出ない（UC-65、[ADR 0285](../decisions/0285-scan-timeouts-are-not-verdicts.md)）。
- **オフライン書出し（CLI-A1）**: ステレオ・u32 フレームまで、ヘルパーは 1 ブロック 60 秒待つ。出力先は元パッケージの外へ固定する（[ADR 0260](../decisions/0260-pinned-gui-export-destination.md)）。
  取消は処理の区切りを待つ。画面・VO は UC-53〜61、聴感は UC-52・57（[ADR 0258](../decisions/0258-peak-export-normalization.md)）。
- **プラグインの取りこぼし**: 欠けた音声・余韻は復元せず、ノートは打ち直す。
  自動ミュートの画面は UC-63・79・86、期限は UC-64、実機・聴感は UC-50・75。
- **プラグインの状態の大きさ**: 上限はない。取得・復元は送信込みで 2 秒待ち、2 GiB 前後から時間切れになる（[ADR 0319](../decisions/0319-control-send-deadline.md)）。
- **ヘルパーの時間切れ（SPG-01）**: 実行中の制御要求があれば、次の要求は busy で拒否する。
  時間切れは結果不明で、背景取得以外は自動再送しない（[ADR 0348](../decisions/0348-background-state-capture-retries.md)）。
  実機・第三者の応答と聴感は UC-28・76・77。
- **プラグインの窓での変更（DOC-A1・A2）**: そのデバイス自身の Undo が、画面の知らない途中の変更を読み込み直すことは残る。
  出力を開き直すと、ヘルパーは記録した状態から作り直される（[ADR 0229](../decisions/0229-plugin-window-live-edits.md)）。
  実画面と第三者プラグインはユーザー確認待ち（UC-21）。
- **背景保存と Undo（DIA-09）**: 背景で取り込んだ状態は現在の版だけに入り、無関係なノートやトラックの Undo でもプラグインが以前の状態へ戻る。
  以前の状態が「なし」ならヘルパーを作り直す。過去の版へ状態を合成しない既存の挙動である（[監査](../reviews/2026-11-13-detailed-implementation-audit.md#dia-09-無関係な編集の-undo-が背景で保存した状態も巻き戻す)）。
- **プラグイン GUI・パラメーターの編集**: デバイスビューの一覧は、プラグインが `clap.params` で公開する値だけを扱う（[ADR 0130](../decisions/0130-clap-parameter-editing.md)）。
  窓が閉じている時のプラグイン自身の変化は、自動保存だけで記録する（[ADR 0132](../decisions/0132-helper-plugin-windows.md)）。
  窓アイコンの実画面・VoiceOver、実ウィンドウの表示、アプリ切替時の隠れ方、第三者プラグインの GUI と聴感はユーザー確認待ち（UC-22）。
- **デバイスビューと GUI の応答性**: 実機と第三者プラグインでの GUI の停止の解消はユーザー確認待ち（UC-23）。
  実画面での暗転の解消はユーザー確認待ち（UC-24、[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
  出力の準備中の編集、一覧の再試行と状態の表示、FX の追加中の再生は UC-43〜46・71。
  再生中の編集の実機は UC-67（[ADR 0330](../decisions/0330-playing-edit-media-and-tempo-retime.md)）。
  取込み中の編集・Undo は取込み後に順に効く（UC-68、[ADR 0331](../decisions/0331-off-worker-audio-import.md)）。
  計画生成は残る。閉じる時は起動中のヘルパーを止める（UC-70、[ADR 0334](../decisions/0334-cancel-helper-starts-on-close.md)）。
  列の開閉と下部の積み方の実画面の操作感と VoiceOver はユーザー確認待ち（UC-26、[ADR 0146](../decisions/0146-independent-editor-stack.md)）。
- **デバイスチェインの表示**: フォルダとデバイス列の開閉は文書内の表示状態で、保存・Undo の対象外である（[ADR 0128](../decisions/0128-folder-disclosure-and-row-projection.md)、[ADR 0137](../decisions/0137-device-disclosure.md)）。
  トラックの複製でオートメーションとルーティングは引き継がない（[ADR 0129](../decisions/0129-track-duplication.md)）。
  狭い幅・低い行・深い字下げでヘッダの FX 名を隠す時は、下部とインスペクターで全チェインを扱う（[ADR 0139](../decisions/0139-track-header-fx-names.md)）。
  インスペクターの FX 追加ボタンの実画面と VoiceOver はユーザー確認待ち（UC-27、[ADR 0165](../decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
- **隔離・Workgroup・キャッシュ**: AUv3 の二重隔離の回避、走査の Seatbelt、Workgroup ポートの 2 ms ポーリングの置換は未実装（[ADR 0111](../decisions/0111-macos-helper-notices.md)）。
  CPU の並列グラフ、メーカー単位のヘルパーの集約、SME、Metal 4 は未実装（[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)）。
