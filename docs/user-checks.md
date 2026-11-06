# ユーザー確認の一覧

自動の回帰では確かめられない、実画面・VoiceOver・聴感・実機・第三者プラグインの確認項目の正本である。
[AGENTS](../AGENTS.md) の「実機・聴感テストの担当」に従い、確認はユーザーが行う。
エージェントは項目を足し、ユーザーの報告を記録する。未確認の項目を合格と扱わない。

## 使い方

- 1 回の確認で複数を閉じられるよう、場面ごとにまとめる。場面の準備をしてから、その場面の項目を順に見る。
- 項目は `UC-番号` で呼ぶ。[現在の制約](status.md) はこの番号を示し、確認待ちの説明をここへ任せる。
- 確認できたら、項目を「確認済み」へ移し、日付と結果を書く。status.md の該当の文も直す。
- 問題が見つかったら、項目に症状を書き残す。エージェントは製品と同じ経路で再現してから直す（[開発手順](development.md)）。
- 新しい確認待ちを作った作業は、同じ作業でここへ項目を足す。番号は使い回さない。
- 追加日は、その確認待ちを status.md に書いた日（`git blame`）か、状態欄に確認待ちと書いた ADR の日付である。
- 観点の略: 画面＝実画面の見え方・操作感、VO＝VoiceOver、聴感＝耳での確認、実機＝音声機器と実時間、第三者＝第三者の CLAP。

## A. アレンジ画面

準備: 音声と MIDI のクリップが複数トラックにあるプロジェクトを開く。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-01 | MIDI クリップの音高表示。中身の音域で高い音ほど上に置き、狭い音域は中央に寄る | 画面・VO | [ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md) | 2026-10-26 |
| UC-02 | ペインのリサイズ中にアレンジが前の絵を伸縮せず、指定したヘッダ幅を保つ | 画面 | [ADR 0144](decisions/0144-unscaled-canvas-resizing.md) | 2026-10-22 |
| UC-03 | 上下のずれ・再生開始時のがくつきがなく、VoiceOver の要素が残る | 画面・VO | [ADR 0149](decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)、[GUI のがくつきの監査](reviews/2026-10-22-gui-jank-audit.md) | 2026-10-23 |
| UC-04 | リサイズ中・直後のちらつきがない | 画面 | [ADR 0150](decisions/0150-stale-drawable-frame.md) | 2026-10-24 |
| UC-05 | クリップの本体ドラッグ・別トラックへの移動・Command の複製・⌘D の操作感 | 画面・VO | [ADR 0154](decisions/0154-clip-move-and-duplicate.md)、[ADR 0188](decisions/0188-arrange-adjacent-clip-duplication.md) | 2026-10-28 |
| UC-06 | ブラウザと Finder からの音声ドロップの実操作と見え方 | 画面・VO | [ADR 0145](decisions/0145-sample-browser-file-drops.md) | 2026-10-22 |

## B. ピアノロール

準備: ノートのある MIDI クリップを 2 トラック以上に置き、ピアノロールを開く。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-07 | 単クリックの記憶長入力の操作感 | 画面 | [ADR 0172](decisions/0172-piano-click-and-input-length.md) | 2026-10-27 |
| UC-08 | 選択した複数ノートのベロシティの一括調整 | 画面・VO・聴感 | [ADR 0177](decisions/0177-piano-batch-note-velocity.md) | 2026-10-27 |
| UC-09 | 編集トラックの切替で時間・音域・倍率を共有し、選択ノートへ自動で移らない | 画面・VO | [ADR 0174](decisions/0174-shared-piano-viewport.md) | 2026-10-27 |
| UC-10 | ノートの一括移動・伸縮・矢印キー・複製ドラッグと、グリッド・三連符・付点・拍の目印 | 画面・VO | [ADR 0178](decisions/0178-piano-batch-note-movement.md)〜[ADR 0187](decisions/0187-piano-independent-time-guides.md) | 2026-10-28 |
| UC-11 | 解放の届かなかった操作（窓の外での解放など）の後、次の押下が前の操作を確定せずに終える | 画面 | [ADR 0206](decisions/0206-piano-interaction-state.md) | 2026-10-31 |
| UC-32 | 単一ノートのドラッグの吸着と Shift での解除、解放時の 1 回の確定 | 画面 | [ADR 0167](decisions/0167-piano-note-drag-snapping.md) | 2026-10-26 |
| UC-33 | ノートの時刻編集で、固定クリップがノート全体を含むまで広がる見え方と音 | 画面・聴感 | [ADR 0168](decisions/0168-note-edit-clip-growth.md) | 2026-10-26 |
| UC-34 | 空白のドラッグでのノートの追加（長さ・音高・Shift・取消） | 画面 | [ADR 0171](decisions/0171-piano-blank-drag-insertion.md) | 2026-10-26 |
| UC-35 | 単一ノートの Option＋上下ドラッグのベロシティ編集と、次の入力への引継ぎ | 画面・VO・聴感 | [ADR 0173](decisions/0173-piano-note-velocity.md) | 2026-10-26 |
| UC-36 | 下部のベロシティレーンの棒の操作 | 画面・VO・聴感 | [ADR 0175](decisions/0175-piano-velocity-lane.md) | 2026-10-26 |
| UC-37 | 右ドラッグの枠選択、Command＋クリックの追加と解除、右クリックの削除と Delete | 画面・VO | [ADR 0176](decisions/0176-piano-note-selection-and-deletion.md) | 2026-10-27 |

## C. ミックスと値の操作子

準備: 複数のトラックを選び、ヘッダとインスペクターを表示する。聴感の項目は再生しながら見る。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-12 | 選択トラックのフェーダーのドラッグ・既定値リセットを全対象で試聴し、1 回の Undo で戻る | 画面・VO・聴感 | [ADR 0228](decisions/0228-selected-track-live-mix.md) | 2026-11-02 |
| UC-13 | 値の操作子のクリック規則（単クリックで変えない・ダブルクリックで既定値・トリプルクリックで入力）。プラグインの表示単位での入力 | 画面・VO・第三者 | [ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md) | 2026-10-25 |
| UC-14 | クリップのゲイン・フェード操作の途中で文書を切り替えても、前の文書の操作が漏れない | 画面・VO | [ADR 0194](decisions/0194-clip-mix-gesture-document-scope.md) | 2026-10-29 |
| UC-38 | 選択トラックのミュート・ソロの一括切替と連打 | 画面・VO・聴感 | [ADR 0189](decisions/0189-selected-track-mute-and-solo.md) | 2026-10-28 |
| UC-39 | ヘッダの音量・パンの VoiceOver の増減（1 dB・5%）を選択全体へ当てる | 画面・VO・聴感 | [ADR 0192](decisions/0192-selected-track-accessibility-steps.md) | 2026-10-28 |
| UC-40 | ヘッダの値の操作の途中で文書を切り替えても、前の文書の操作が漏れない | 画面・VO | [ADR 0193](decisions/0193-header-mix-gesture-document-scope.md) | 2026-10-29 |
| UC-49 | 入れ子のフォルダと外側の音源・音声を再生し、親のミュートで子孫が止まり、親のソロで子孫だけが聴こえる。子のミュート優先・複数ソロ・解除・親の付替えでも、不自然な段差がなく各行の保存値と操作対象を読み分けられる | 画面・VO・聴感 | [ADR 0244](decisions/0244-folder-mute-and-solo-gates.md) | 2026-11-03 |

## D. ブラウザと試聴

準備: 音声機器へ出力し、サンプルのフォルダを登録する。曲の再生と試聴を重ねる項目がある。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-15 | 一覧のクリックの遅れとフォーカスが直った | 画面 | [ADR 0143](decisions/0143-browser-click-and-audition-timing.md) | 2026-10-22 |
| UC-16 | 試聴と曲の再生の 2 つの IOProc の加算、機器の切替時の挙動 | 実機・聴感 | [ADR 0135](decisions/0135-sample-audition.md) | 2026-10-21 |
| UC-41 | 長い素材を全部読む前に試聴が始まり、読込み中は部分の波形が出る | 実機・聴感・画面 | [ADR 0151](decisions/0151-streaming-sample-audition.md) | 2026-10-24 |
| UC-42 | ブラウザ表示中の試聴出力の保持、音量、自動試聴の切替、右矢印と波形クリックでの試聴の開始時間 | 実機・聴感・画面 | [ADR 0152](decisions/0152-browser-audition-controls.md) | 2026-10-24 |

## E. トランスポートと再生

準備: 音声機器へ出力し、プラグインのない曲とある曲を用意する。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-17 | 停止で記憶した開始位置へ戻る操作感と、その時の音 | 画面・聴感 | [ADR 0155](decisions/0155-remembered-playback-start.md) | 2026-10-25 |
| UC-18 | 再生ヘッドの追従のページ送りの見え方と、トグルの VoiceOver | 画面・VO | [ADR 0159](decisions/0159-playhead-follow.md) | 2026-10-25 |
| UC-19 | 開いた直後の最初の再生の待ちが減った量と操作感 | 実機・画面 | [ADR 0147](decisions/0147-output-prewarm-and-background-media.md) | 2026-10-22 |
| UC-20 | 2026-10-25 の報告（開始位置の変更時の暗転、一覧での Space、ブラウザの波形、自動フェード、FX 候補、ノートの終端からの再生）が直った | 画面・聴感 | [照合](reviews/2026-10-25-user-feedback.md) | 2026-10-26 |
| UC-47 | 多数のノートがある曲を短区間で繰り返し、長く再生した後のノート編集・位置移動でも音切れ・余韻の不自然な再開・鳴りっぱなしがない。機器・バッファ長・プラグインと取りこぼし数を記録する | 実機・聴感・第三者 | [ADR 0239](decisions/0239-live-note-cursor-work.md)、[ADR 0240](decisions/0240-block-local-event-capacity.md) | 2026-11-03 |
| UC-48 | 長い音声と短いトリム、モノとステレオが混在するトラックでクリップゲインを変更し、音量・フェード・ループの聴感が保たれ、操作の待ちが問題ない。機器・素材長・バッファ長を記録する | 実機・聴感 | [ADR 0241](decisions/0241-unscaled-placement-pcm.md) | 2026-11-03 |
| UC-62 | 原点の前後に分数テンポの点を置き、位置移動・再生・メトロノームで表示位置とクリックが一致する。機器ごとのバッファ長を変えても段差やずれが増えない。機器・サンプルレート・バッファ長を記録する | 実機・聴感 | [ADR 0275](decisions/0275-shared-integer-tempo-index.md) | 2026-11-06 |

## F. プラグイン

準備: 第三者の CLAP（窓を持つ音源とエフェクト）をトラックに入れ、音声機器へ出力する。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-21 | 窓での変更（パラメーターにならない編集を含む）が、無関係な編集の後も残り、1 回の Undo で戻る | 画面・第三者 | [ADR 0229](decisions/0229-plugin-window-live-edits.md) | 2026-11-02 |
| UC-22 | 窓アイコンと VoiceOver、実ウィンドウの表示、アプリ切替時の隠れ方、第三者の GUI と音 | 画面・VO・第三者・聴感 | [ADR 0130](decisions/0130-clap-parameter-editing.md)、[ADR 0132](decisions/0132-helper-plugin-windows.md)、[ADR 0134](decisions/0134-plugin-window-lifetime-and-shortcuts.md) | 2026-10-21 |
| UC-23 | プラグイン付きトラックを選ぶたびの GUI の停止と、パラメーター画面の重さが直った | 実機・第三者 | [ADR 0131](decisions/0131-device-view-and-responsive-parameters.md) | 2026-10-20 |
| UC-24 | パラメーターの確定・バイパス・オフラインの操作で画面全体が暗転しない | 画面 | [ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md) | 2026-10-21 |
| UC-25 | 停止中もプラグインの余韻とプラグイン内の鍵盤の音が出る | 実機・第三者・聴感 | [ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md) | 2026-10-21 |
| UC-26 | 下部の列の開閉と積み方の操作感、VoiceOver。複数選択時の表示対象は未決 | 画面・VO | [ADR 0146](decisions/0146-independent-editor-stack.md) | 2026-10-22 |
| UC-27 | インスペクターの一覧末尾の FX 追加ボタン | 画面・VO | [ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md) | 2026-10-26 |
| UC-28 | ヘルパーの時間切れの実機での期限と、完了後の発音 | 実機・聴感 | [ADR 0199](decisions/0199-owned-helper-control-requests.md) | 2026-10-30 |
| UC-43 | 停止中の最初の出力とヘルパーの準備の間も、編集と操作が待たされない | 実機・画面 | [ADR 0136](decisions/0136-output-opening-off-worker.md) | 2026-10-21 |
| UC-44 | パラメーター一覧の読込みに失敗した時、デバイス列から再試行できる | 画面・VO | [ADR 0140](decisions/0140-device-parameter-list-retry.md) | 2026-10-21 |
| UC-45 | デバイス列が、出力の準備中と一覧の応答待ちを区別して表示する | 画面・VO | [ADR 0141](decisions/0141-device-output-preparation-status.md) | 2026-10-21 |
| UC-46 | 開いた出力への FX 追加・オフライン解除・コピーの間も編集が続き、音が途切れない | 実機・聴感 | [ADR 0142](decisions/0142-background-plugin-graph-preparation.md) | 2026-10-21 |
| UC-50 | プラグインが実時間の処理に遅れた時と復帰時に、短い減衰でクリックが和らぎ、別トラックは途切れない。自動の PCM 比較では実機の遅れ方と聴感は分からない | 実機・第三者・聴感 | [ADR 0245](decisions/0245-plugin-deadline-fades.md) | 2026-11-03 |
| UC-51 | 遅延を持つ CLAP を挿し、出力レートの報告値と未補正が列に表示され、設定変更の再起動で更新される。列の開閉と VO の読みを確認する。自動の模擬プラグインでは第三者の報告値と実画面は分からない | 画面・VO・第三者 | [ADR 0246](decisions/0246-plugin-latency-reporting.md) | 2026-11-03 |

## G. 文書とプロジェクト

準備: 2 つのプロジェクトを用意する。UC-30 は、壊した複製で試す（元のプロジェクトでは試さない）。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-29 | 文書を開く・閉じる・切り替える間の画面、VoiceOver、音 | 画面・VO・聴感 | [ADR 0203](decisions/0203-document-session.md) | 2026-10-31 |
| UC-30 | 古い保存版へ戻す復旧の警告の見え方と VoiceOver | 画面・VO | [ADR 0183](decisions/0183-recovery-consent.md) | 2026-10-28 |
| UC-31 | 前回のプロジェクトの復元に失敗した後、別のプロジェクトを開ける（同じ後の新規作成とトラックの追加は 2026-10-19 にユーザーが確認した。[ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)） | 画面 | [現在の制約](status.md) のプロジェクトの開始 | 2026-10-19 |

## H. 書出し

準備: 小音量の音と余韻を持つ曲を、CLI の PCM16 / PCM24 で `--dither none` と `--dither tpdf` の別ファイルへ書き出す。

| ID | 確認すること | 観点 | 根拠 | 追加日 |
|---|---|---|---|---|
| UC-52 | 普段の読み手で開け、小音量と余韻が不自然に途切れず、ノイズが意図した程度である。自動の数値・復号比較では聴感と読み手全般の互換性は分からない | 聴感 | [ADR 0251](decisions/0251-tpdf-export-dither.md) | 2026-11-03 |
| UC-53 | ファイルメニュー・⇧⌘E から形式・レート・余韻・ディザを指定し、再オープン後も設定が戻る。書出し中の編集・再生・文書切替、結果とエラーの読み上げを確認する。自動回帰ではシート・保存パネル・VO の操作感は分からない | 画面・VO | [ADR 0254](decisions/0254-gui-snapshot-export.md) | 2026-11-03 |
| UC-54 | 長い曲の書出しで準備中・処理済みの率・仕上げ中・完了／失敗を区別でき、文書切替後も同じジョブの表示が続く。バーと率の見え方・VO の読みを確認する。自動回帰では実画面の操作感と読み上げは分からない | 画面・VO | [ADR 0255](decisions/0255-export-progress.md) | 2026-11-03 |
| UC-55 | 準備中・処理中の取消ボタン、取消中の連打無効・取消済みの表示と VO。文書切替後も同じ書出しを取り消せること、完了が先なら完了を表示すること。自動回帰ではボタンの操作感と読み上げは分からない | 画面・VO | [ADR 0256](decisions/0256-export-cancellation.md) | 2026-11-04 |
| UC-56 | 書出し後にパッケージを移動・コピーし、再オープンした設定シートへ前回値が戻る。読込み待ちと VoiceOver、旧版の設定からの移行を確認する。自動回帰では実画面の待ちの見え方と読み上げは分からない | 画面・VO | [ADR 0257](decisions/0257-portable-export-settings.md) | 2026-11-04 |
| UC-57 | 書出しシートのピークノーマライズの切替・目標入力・前回値と VO、音量が意図どおりになるかを確認する。−1 dBFS の有無と TPDF を別ファイルで比較する。自動回帰では入力の操作感・読み上げ・聴感は分からない | 画面・VO・聴感 | [ADR 0258](decisions/0258-peak-export-normalization.md) | 2026-11-04 |
| UC-58 | 完了後の Finder 表示・文書交換・連打と VO。自動回帰は実操作を代用しない | 画面・VO | [ADR 0261](decisions/0261-reveal-export-output.md) | 2026-11-04 |
| UC-59 | 数値欄を空・文字混じり・範囲外にして、入力中も開始が無効になり、理由が読めることを確認する。修正、余韻・ノーマライズの切替、小数点のロケールと VO も確認する。自動回帰では実画面の配置・読み上げは分からない | 画面・VO | [ADR 0262](decisions/0262-export-numeric-drafts.md) | 2026-11-04 |
| UC-60 | 共通既定設定の保存・適用・初期化と、前回設定のない別プロジェクトへの適用を確認する。日英の配置・キーボード操作・完了メッセージと VO は自動回帰では判断できない | 画面・VO | [ADR 0263](decisions/0263-common-export-defaults.md) | 2026-11-04 |
| UC-61 | 名前付き書出しプリセットの開閉・保存・選択・適用・削除と、同名の保存拒否を確認する。日英の配置・長い名前・キーボード操作・VO は非表示の自動回帰では判断できない | 画面・VO | [ADR 0264](decisions/0264-named-export-presets.md) | 2026-11-05 |

## 確認済み

まだない。2026-10-17 の Phase 1 の聴感確認は [最終照合](reviews/2026-10-17-phase1-acceptance.md) にある。
