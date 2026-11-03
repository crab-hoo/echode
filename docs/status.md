# 現在の範囲と制約

更新: 2026-11-03。
利用者に今も残る制約と、未確認・未接続の項目だけを置く。最新ソースの合格証ではない。
合否は `make verify` の結果、実操作は日付付きの監査記録で確認する。
実装の経緯は ADR、Phase 2 の単位ごとの進み具合と受入条件は [Phase 2 の進め方](phase2.md)、監査の対応は各監査にある。
ここへ経緯を書き足さず、制約が変わった時だけ該当する項目を書き換える。
ユーザーの確認待ちは `UC-番号` で示し、内容と手順は [ユーザー確認の一覧](user-checks.md) にある。

対象は macOS 27 以降、M4 ファミリー以降の Apple Silicon である（[ADR 0112](decisions/0112-m4-minimum.md)、[ADR 0114](decisions/0114-m4-build-baseline.md)）。
起動時のチップ名による機種判定と SME 最適化は未実装で、実機性能の改善量は未確認である。

## 進捗

- **Phase 0（S1〜S5）**: 確定済み。
- **Phase 1**: 完了（[最終照合](reviews/2026-10-17-phase1-acceptance.md)）。
- **Phase 2**: 進行中。単位ごとの状態・受入条件・関連 ADR は [進め方](phase2.md) にある。
  - **P2-01（ヘッドレス MCP）**: 完了。GUI アプリへの接続・再生操作・外部クライアントの登録は未接続（[ADR 0115](decisions/0115-phase2-mcp.md)）。
  - **P2-02（デバイスビューとチェイン操作）**: 着手中。オートメーションは未接続で、P2-04 で扱う。
  - **P2-06（ミックス）**: 先行分だけを接続した。コンソール表示は未接続（[ADR 0228](decisions/0228-selected-track-live-mix.md)）。
  - **P2-07（マーカー・録音・ピアノロール）**: 着手中。
    複数トラックの同時編集・試聴・ドラム入力は未接続（[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)、[ADR 0184](decisions/0184-piano-note-duplicate-drag.md)）。
- **監査記録**: 2026-10-08 の設計・計測の未着手項目は [元の一覧](reviews/2026-10-08-audit.md) が正本である。
  後続の監査で指摘がなかったことを理由に完了へ変えない。

## 利用時に残る制約

### 保存・復旧・パッケージ

- **保存のクラッシュ耐久性（XCA-01〜03、HIS-A1）**: 連番の欠けと状態の保管物の欠落は、読めない先端と同じく同意まで書込みを拒否する。
  作業ツリーの JSON は同期しない。実際の電源断は未実施で、Swift の警告は原因をまだ言い分けない（[ADR 0230](decisions/0230-save-durability.md)）。
- **復旧と保存の信頼境界（2026-10-27 監査）**: ジャーナル適用後に不変条件が壊れていれば、連番と理由を返してオープンを拒否する（ADV-01、[ADR 0180](decisions/0180-journal-replay-invariants.md)）。
  読めない先端から親へ戻る復旧は、明示の同意まで書込みを拒否する（ADV-02、[ADR 0183](decisions/0183-recovery-consent.md)）。
  失われた編集は復元しない。警告の実画面・VoiceOver はユーザー確認待ち（UC-30）。
  消失済みの過去の完全ログ・画像は復元していない（[証拠の保全](evidence/README.md)）。
- **プロジェクトの開始**: 前回プロジェクトの復元に失敗した後の実画面の操作はユーザー確認待ち（UC-31）。
  最初に Git の初期化が失敗した理由は未確定である（[ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)）。
- **パッケージの浮動小数点の精度**: 既に丸められた保存値は復元しない。スキーマは 6 のまま（[ADR 0196](decisions/0196-lossless-package-floats.md)）。
- **素材とキャッシュの Git 除外**: 任意の Git パターンの正規化はしない（[ADR 0031](decisions/0031-content-addressed-store.md#除外規則の行単位の照合2026-10-29)）。
- **保存済み音声の有限性**: 保存済み PCM と外部音声の取込みは、NaN・無限大を含めば拒否する。
  波形キャッシュのマップ読込みと実機の聴感は、この検証の範囲外である（[ADR 0056](decisions/0056-audio-import.md#外部音声の一括復号と取込みの検証2026-10-29)）。
- **波形キャッシュの破損**: 壊れたキャッシュは元の PCM から作り直し、素材が読めなければ波形を出さない。
  素材に対する全列の認証はしない。旧 `ECHDPK01` は作り直し、現在の識別子は `ECHDPK02` である（[ADR 0058](decisions/0058-waveform-peaks.md)）。

### API・MCP・文書の入口

- **JSON-RPC の不正な要求**: 真偽値・配列・オブジェクトの ID とスカラーの params は、編集前に拒否する。
  版やメソッドの不正は、ID がなくても `-32600` を返す（[ADR 0020](decisions/0020-json-rpc.md#要求の外枠の検証2026-10-29)）。
  JSON 外の空白を前後に付けた入力は、編集前に `-32700` で拒否する（[ADR 0020](decisions/0020-json-rpc.md#json-の空白だけを除去する2026-10-29)）。
  改行前に切断した要求は編集せず、接続を閉じる（[ADR 0073](decisions/0073-socket-path.md#改行前の切断の拒否2026-10-29)）。
- **API の応答の書込み**: 応答の書込みが 30 秒進まない接続は閉じる。送信失敗の前に成立した編集は再実行しない（[ADR 0073](decisions/0073-socket-path.md)）。
- **MCP の接続先の不正なエラー応答**: 不正な error 応答は結果不明として、再送前の確認を案内する。
  確定済みの編集を再送・巻戻ししない（[ADR 0115](decisions/0115-phase2-mcp.md#接続先のエラー応答の形の検証2026-10-29)）。
- **文書の寿命と古い入力（SPG-02）**: 閉じる間はヘルパーの終了を待たず、文書なしの画面を `busy` のまま表示する（[ADR 0203](decisions/0203-document-session.md)）。
  実画面での文書切替・VoiceOver・聴感はユーザー確認待ち（UC-29）。
- **文書の手番の調停（SPG-05）**: CLI と MCP はこのサービスを使わず、`DocumentHost` へ直接つなぐ（[ADR 0204](decisions/0204-document-service.md)）。
- **Swift の型付き編集入口（SPG-03）**: 互換の raw JSON の入口が残る（[ADR 0202](decisions/0202-generated-swift-edit-actions.md)）。

### 音声・再生・トランスポート

- **停止と一時停止**: 停止は記憶した開始位置へ戻り、一時停止はその場に保つ（[ADR 0155](decisions/0155-remembered-playback-start.md)）。
  Return は、入力欄とブラウザ一覧の確定・配置を優先する（[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)）。
  実画面の操作感と、停止で戻る時の聴感はユーザー確認待ち（UC-17）。
- **再生ヘッドの追従**: 追従は保存しない表示状態で、キーの割当はない。
  停止中のズームで範囲外になった再生ヘッドは、次の再生・停止・位置移動まで表示しない（[ADR 0159](decisions/0159-playhead-follow.md)）。
  ページ送りの見え方とトグルの VoiceOver はユーザー確認待ち（UC-18）。
- **開いた時の出力準備**: 開いた直後の準備が終わる前の再生は、準備の残りを待つ。
  開いた出力は停止中もプラグインを処理するので、その負荷は開いた時点から生じる（[ADR 0147](decisions/0147-output-prewarm-and-background-media.md)）。
  実機での待ちの改善量と操作感はユーザー確認待ち（UC-19）。
- **ライブ停止**: 停止中も無音の入力でプラグインを処理し、余韻とプラグイン内の鍵盤の音を出力する。
  開いた出力の停止中の処理負荷は残る（[ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)）。実機と第三者プラグインの聴感はユーザー確認待ち（UC-25）。
- **位置移動と準備の取消**: 位置移動・未発音の準備の取消とオフラインの停止では、以前の発音状態を消す（[ADR 0099](decisions/0099-transport-plugin-state.md)）。
- **停止診断**: 停止診断の減衰位置は素材側を示し、マスター出力の無音化を示さない（[ADR 0104](decisions/0104-stop-output-diagnostics.md)、[ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
- **バス・ループ・ゲイン・メーター**: 暗黙のステレオ→モノ変換はエラーにする（[ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)）。
  ディスクストリーミング、Phase 3 のレンダリングキャッシュの生成本体、Ogg、プラグインのサムネイルは未実装（[ADR 0108](decisions/0108-phase1-loop-readiness.md)）。
  クリップゲインは `0..=1_048_576`（音声クリップのみ）で、負の配置オフセットはファイル前の無音として扱う（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
  トラックメーターはフェーダー後の左右最大ピークで、LUFS やトゥルーピークではない（[ADR 0107](decisions/0107-mix-controls.md)）。
- **Sine Pad と音源の選択**: 音源のないトラックのノートは無音で、旧プロジェクトへ暗黙の音源を補わない。
  従来の既定音を使うには Sine Pad を挿す（[ADR 0123](decisions/0123-explicit-sine-pad-plugin.md)）。
- **規模とミックスの意味（FCA-01・02・06・07）**: ノートの多いプロジェクトでは、ループの折り返しと再生中の編集で音が切れ得る。
  フォルダの音量・ミュート・ソロは子に効かず、プラグインの遅延は補正しない（[監査](reviews/2026-11-03-full-code-audit.md)）。
- **Phase 0 の別モデル・計測の分離（SPG-10）**: 分離は完了したが、並列の CPU グラフを製品へ接続したものではない（[ADR 0211](decisions/0211-opt-in-history-spike.md)、[ADR 0213](decisions/0213-opt-in-engine-spike.md)）。

### プラグイン・デバイスビュー

- **CLAP バンドルの互換性**: 音源とエフェクトは同じ隔離チェインを通る。macOS のディレクトリ形式のバンドルも走査・処理できる。
  全メーカーの互換性と実機の聴感は保証しない（[ADR 0093](decisions/0093-product-contract-gate.md)）。
- **プラグインを使うオフライン書き出し（CLI-A1）**: 応答しなくなったヘルパーは、1 ブロック 60 秒で書き出しを止める。
  走査の出力には上限がある（[ADR 0231](decisions/0231-offline-export-and-scan-cache.md)）。
- **ヘルパーの時間切れ（SPG-01）**: 実行中の制御要求があれば、次の要求は busy で拒否する。
  時間切れは結果不明で、プラグインの実行自体の取消・自動再送はしない（[ADR 0199](decisions/0199-owned-helper-control-requests.md)）。
  実機の期限と聴感はユーザー確認待ち（UC-28）。
- **プラグインの窓での変更（DOC-A1・A2）**: そのデバイス自身の Undo が、画面の知らない途中の変更を読み込み直すことは残る。
  出力を開き直すと、ヘルパーは記録した状態から作り直される（[ADR 0229](decisions/0229-plugin-window-live-edits.md)）。
  実画面と第三者プラグインはユーザー確認待ち（UC-21）。
- **プラグイン GUI・パラメーターの編集**: デバイスビューの一覧は、プラグインが `clap.params` で公開する値だけを扱う（[ADR 0130](decisions/0130-clap-parameter-editing.md)）。
  窓が閉じている時のプラグイン自身の変化は、自動保存だけで記録する（[ADR 0132](decisions/0132-helper-plugin-windows.md)）。
  窓アイコンの実画面・VoiceOver、実ウィンドウの表示、アプリ切替時の隠れ方、第三者プラグインの GUI と聴感はユーザー確認待ち（UC-22）。
- **デバイスビューと GUI の応答性**: 実機と第三者プラグインでの GUI の停止の解消はユーザー確認待ち（UC-23）。
  複数選択時のデバイスビューの表示対象は未決（[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）。
  実画面での暗転の解消はユーザー確認待ち（UC-24、[ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
  出力の準備中の編集、一覧の再試行と状態の表示、FX の追加中の再生もユーザー確認待ち（UC-43〜46）。
  再生中の編集の素材と、保存済み状態の読込みは同期のまま残る（[ADR 0147](decisions/0147-output-prewarm-and-background-media.md)）。
  初回導入時の差分による追加と、文書終了時の初期化待ちは残る（[ADR 0142](decisions/0142-background-plugin-graph-preparation.md)）。
  列の開閉と下部の積み方の実画面の操作感と VoiceOver はユーザー確認待ち（UC-26、[ADR 0146](decisions/0146-independent-editor-stack.md)）。
- **デバイスチェインの表示**: フォルダとデバイス列の開閉は文書内の表示状態で、保存・Undo の対象外である（[ADR 0128](decisions/0128-folder-disclosure-and-row-projection.md)、[ADR 0137](decisions/0137-device-disclosure.md)）。
  再オープン時はフォルダを展開に戻す。トラックの複製でオートメーションとルーティングは引き継がない（[ADR 0129](decisions/0129-track-duplication.md)）。
  狭い幅・低い行・深い字下げでヘッダの FX 名を隠す時は、下部とインスペクターで全チェインを扱う（[ADR 0139](decisions/0139-track-header-fx-names.md)）。
  インスペクターの FX 追加ボタンの実画面と VoiceOver はユーザー確認待ち（UC-27、[ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
- **隔離・Workgroup・キャッシュ**: AUv3 の二重隔離の回避、走査の Seatbelt、Workgroup ポートの 2 ms ポーリングの置換は未実装（[ADR 0111](decisions/0111-macos-helper-notices.md)）。
  CPU の並列グラフ、メーカー単位のヘルパーの集約、SME、Metal 4 は未実装（[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)）。

### ピアノロール

- **入力長**: 記憶した入力長は全トラック共有で、再オープン時に初期化される（[ADR 0172](decisions/0172-piano-click-and-input-length.md)）。
  実画面の操作感はユーザー確認待ち（UC-07）。
- **複数ノートのベロシティ**: 一括調整の実画面・VoiceOver・聴感はユーザー確認待ち（UC-08、[ADR 0177](decisions/0177-piano-batch-note-velocity.md)）。
- **表示範囲の共有**: 実画面と VoiceOver はユーザー確認待ち（UC-09、[ADR 0174](decisions/0174-shared-piano-viewport.md)）。
- **ノートの編集**: 吸着・追加・ベロシティ・枠選択と、選択・移動・複製・グリッドの実画面・VoiceOver・聴感はユーザー確認待ち（UC-10、UC-32〜37）。
- **操作の状態の一本化（SPG-08）**: 解放が届かない製品の経路は未確認で、実画面はユーザー確認待ち（UC-11、[ADR 0206](decisions/0206-piano-interaction-state.md)）。
- **クリップの大きさ**: 旧ジャーナルの移動・複製は、従来の固定化の規則で再生する（[ADR 0170](decisions/0170-content-relative-auto-bars.md)）。
  ノートの所属替えと、隣のクリップで止める処理は未接続で、草案に残る（[ADR 0169](decisions/0169-clip-growth-follows-notes-and-stops-at-walls.md)）。

### アレンジ・ブラウザ・ミックス・画面

- **MIDI クリップの音高表示**: 実画面と VoiceOver はユーザー確認待ち（UC-01、[ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
- **クリップの移動と複製（UIF-03）**: 移動先で重なる配置は置き換え（削除・切詰め・分割）になる（[ADR 0154](decisions/0154-clip-move-and-duplicate.md)）。
  ⌘D はアレンジにフォーカスがある時だけ有効で、押し続けの自動反復はしない（[ADR 0188](decisions/0188-arrange-adjacent-clip-duplication.md)）。
  重ねるモード・自動クロスフェード・複数クリップは未接続。実画面の操作感と VoiceOver はユーザー確認待ち（UC-05）。
- **クリップのミックス操作の文書交換**: 実画面の文書切替・VoiceOver はユーザー確認待ち（UC-14、[ADR 0194](decisions/0194-clip-mix-gesture-document-scope.md)）。
- **音声のドロップ**: 複数ファイルの同時ドロップ・自動スクロール・配置予定の波形と、同じトラックでの配置の重複は未接続（[ADR 0145](decisions/0145-sample-browser-file-drops.md)）。
  ドラッグの実操作・見え方・VoiceOver はユーザー確認待ち（UC-06）。
- **サンプルブラウザのクリック（UIF-05・09）**: クリックの遅れとフォーカスが直ったかはユーザー確認待ち（UC-15）。
  遅れの量は未計測である（[ADR 0143](decisions/0143-browser-click-and-audition-timing.md)）。
- **サンプルの試聴**: 選択時の試聴は素の音で、プラグインのヘルパーを起動しない（[ADR 0135](decisions/0135-sample-audition.md)）。
  表示中は無音の試聴出力を保つ（[ADR 0152](decisions/0152-browser-audition-controls.md)）。
  隣の行の先読みと、長いファイルの読込み時間の計測はしていない（[ADR 0163](decisions/0163-sample-overview.md)）。
  2 つの IOProc の加算と機器切替時の挙動、聴感は実機でのユーザー確認待ち（UC-16、[ADR 0135](decisions/0135-sample-audition.md)）。
  長い素材の読込み中の試聴と、表示中の試聴の操作もユーザー確認待ち（UC-41・42）。
  試聴のループ（UIF-10）は再現しておらず、原因は未確定である（[照合](reviews/2026-10-21-ui-feedback-audit.md)）。
- **2026-10-25 の操作の報告**: サンプル一覧の Space が製品のどこで止まっていたかは再現していない（[照合](reviews/2026-10-25-user-feedback.md)）。
  実画面と聴感での解消はユーザー確認待ち（UC-20）。
- **選択トラックのミックス（P2-06）**: ピアノロールの行の Solo / Mute ボタンは単一トラックのまま。選択外のソロは解除しない（[ADR 0189](decisions/0189-selected-track-mute-and-solo.md)）。
  無音の基準からの音量比が定まらない一括入力は、理由を示して拒否する（[ADR 0191](decisions/0191-selected-track-numeric-mix.md)）。
  一括の切替・読み上げの増減・文書切替・ドラッグとリセットの実画面・VoiceOver・聴感はユーザー確認待ち（UC-12、UC-38〜40、[ADR 0228](decisions/0228-selected-track-live-mix.md)）。
- **値の操作子（UIF-04）**: 実画面の操作感・VoiceOver・第三者プラグインの変換はユーザー確認待ち（UC-13、[ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md)）。
- **ペインのリサイズと描画（UIF-01、JNK-01・02）**: リサイズ中の見え方はユーザー確認待ち（UC-02、[ADR 0144](decisions/0144-unscaled-canvas-resizing.md)）。
  上下のずれ・再生開始時のがくつき・VoiceOver の解消はユーザー確認待ち（UC-03、[ADR 0149](decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
  ちらつきの有無はユーザー確認待ち（UC-04、[ADR 0150](decisions/0150-stale-drawable-frame.md)）。
  分割線ドラッグの計測（単位 6、JNK-05）は未着手（[GUI のがくつきの監査](reviews/2026-10-22-gui-jank-audit.md)）。
