# 現在の範囲と制約

更新: 2026-10-20。
製品の完成状況を案内する文書であり、最新ソースの合格証ではない。
合否は `make verify` の結果、実操作は日付付きの監査記録で確認する。

対象方針は macOS 27 以降、M4 ファミリー以降の Apple Silicon（[ADR 0112](decisions/0112-m4-minimum.md)）。
Rust・C / C++・Swift と補助コードの M4 向けコンパイル設定、製品・依存・検査の最低 OS 27.0 を接続した（[ADR 0114](decisions/0114-m4-build-baseline.md)）。
チップ名による起動時の機種判定と SME 最適化、実機性能の改善量は未確認である。

## 進捗

- **Phase 0（S1〜S5）**: 確定済み。
- **Phase 1**: 完了（[最終照合](reviews/2026-10-17-phase1-acceptance.md)）。
  モデルとアクション、保存・復旧、CLI / API、音声出力、CLAP の隔離、3 ペインの開発用アプリまで接続した。
  履歴復帰、音声取り込み、Metal のアレンジとピアノロール、トラックの音量・パン・ミュート・ソロ・ピークと音声クリップのゲイン・フェード（[ADR 0107](decisions/0107-mix-controls.md)）、ループ先頭の保持とフェーダー前の差し替え口（[ADR 0108](decisions/0108-phase1-loop-readiness.md)）を製品へ接続し、2026-10-17 にユーザーの聴感確認を受領した。
- **監査記録**: 2026-10-13 の AUD / PRO の修正は各監査の対応欄と ADR に残す。
  2026-10-08 の設計・計測の未着手項目は元の一覧が正本で、後続の監査で指摘がなかったことを理由に完了へ変えない。
- **Phase 2（進行中）**: [進め方](phase2.md) に実装順と各単位の受入条件を置く。
  - **P2-01（ヘッドレス MCP）**: 完了。既存のヘッドレスセッションへ接続する `echode mcp SOCKET` を実装し、アクション登録簿からのツール生成、単一 Undo の編集、Undo / Redo、概要とトラックのリソースを接続した（[ADR 0115](decisions/0115-phase2-mcp.md)）。GUI アプリへの接続・再生操作・外部クライアント登録は未接続。
  - **P2-02（デバイスビューとチェイン操作）**: 着手中。
    トラックの FX ボタン、下部の FX ビュー、非モーダルの候補パネル、同一チェインの並べ替え・Command 複製・別トラックへのコピーを保存・Undo / Redo・再読込へ接続した（[ADR 0118](decisions/0118-track-fx-chain-editing.md)）。
    CLAP のホストバイパスも FX 行から保存・Undo・再生・書出しへ接続した（[ADR 0120](decisions/0120-device-bypass.md)）。
    FX 行のオフライン切替を記録済み設定の保持・隔離グラフからの除去・再生成・保存形式 5 へ接続した
    （[ADR 0125](decisions/0125-device-offline.md)）。
    ヘッダの複数選択と候補の右メニューからの全選択トラックへの一括追加を、単一 Undo・保存へ接続した
    （[ADR 0124](decisions/0124-selected-track-fx-batches.md)）。
    共通サンプルブラウザの登録フォルダ全体の名前検索と、操作時のトラック・ライブ位置を保持する配置を接続した
    （[ADR 0122](decisions/0122-sample-library-search-and-placement.md)）。選択時の単発試聴は未実装。
    稼働中の CLAP のパラメーター一覧・ドラッグ中の試聴・終了時の単一 Undo を接続した。
    値はプラグイン状態として記録し、Undo / Redo は同じヘルパーへ状態を戻す。停止中は再生せずに出力とヘルパーを開く
    （[ADR 0130](decisions/0130-clap-parameter-editing.md)）。
    下部は選択トラックのデバイスを横に並べ、各列に全パラメーターの一覧・検索・ピン留めを持つ。
    一覧は画面内の行だけを作り、ピン留めは保存形式 6 でデバイスごとに記録する。
    一覧・値の往復は文書ワーカーの外で待ち、選択と無関係な編集はプラグインの応答を待たない
    （[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）。
    プラグイン GUI・プラグイン側の値変更の中継・オートメーションは未接続。
    トラック追加は、コマンドでは選択の下（展開フォルダは最後の子）、一覧末尾のボタンでは最上位末尾へ接続した。
    単一 Undo と保存再読込を持つ
    （[ADR 0127](decisions/0127-positioned-track-insertion.md)）。
    フォルダの開閉、表示行に沿った選択・ミックス・FX ドロップと、折りたたみ中の部分木の外側への追加も接続した。
    開閉は保存・Undo の対象外で、再オープン時は展開に戻る
    （[ADR 0128](decisions/0128-folder-disclosure-and-row-projection.md)）。
    トラックの通常複製を、メニュー・パレット・ヘッダの右メニューから部分木単位で接続した。
    音源・FX・クリップ・ミキサー値を写し、中身とノートは元と独立させ、単一 Undo・保存へつなぐ
    （[ADR 0129](decisions/0129-track-duplication.md)）。オートメーションとルーティングの引き継ぎは各単位で扱う。

## 利用時に残る制約

- **デバイスビューと GUI 応答性**:
  2026-10-20 に報告された、プラグイン付きトラックを選ぶたびの GUI 停止とパラメーター画面の重さは、機器なしで再現した。
  旧欄が全パラメーターの行を一括生成していたこと（800 個で約 0.68 秒、3000 個で約 2.7 秒）と、デバイス要求が文書ワーカーを塞いでいたことに対応した。
  実機と第三者プラグインでの解消はユーザー確認待ちである
  （[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）。
  停止中の最初の表示で出力とヘルパーを開く間は、後続の編集がまだ待つ。
  展開・折りたたみ、名前クリックでの独自 GUI、複数選択時の表示対象は未実装・未決である
  （[デバイスビュー](interaction-design.md#デバイスビューと-fx-一覧の役割)、[作業継続](interaction-design.md#処理中も作業を続ける)）。
- **プロジェクトの開始**: 起動直後も「新規プロジェクト」と「開く」を利用できる。
  前回プロジェクトの復元で Git 準備に失敗すると「変更を保存中」のまま止まる不具合を修正し、エラーを返して新規作成へ戻れるようにした。
  エラー後の有効化と別プロジェクトの作成は機器・ウィンドウなしの回帰で確認し、実画面の操作確認はユーザー担当とする。
- **プラグイン GUI・パラメーター編集**: GUI は未実装。デバイスビューの一覧はプラグインが `clap.params` で公開する値だけを扱う。
  プラグイン GUI で変えた値は自動保存で取り込まれるが、欄への即時反映と Undo への記録は未接続である。
  実機での第三者プラグインの操作・聴感は未確認（[設計 §9.5](ARCHITECTURE.md#95-gui-とデバイスビュー)、[ADR 0130](decisions/0130-clap-parameter-editing.md)）。
- **Sine Pad と音源の選択**: Sine Pad は同梱 CLAP として通常の FX 一覧から追加・削除・バイパスできる。
  音源のないトラックのノートは無音とし、旧プロジェクトへ暗黙の音源を補わない。
  従来の既定音を使う場合は Sine Pad を挿す（[ADR 0123](decisions/0123-explicit-sine-pad-plugin.md)）。
- **CLAP バンドル互換性**: 音源とエフェクトは同じ隔離チェインを通る。macOS のディレクトリ形式バンドルも走査・処理できるが、全メーカーの互換性や実機の聴感を保証するものではない。
- **トランスポートと状態保持**:
  - 再生開始時の位置巻き戻り（CU-04）とトラックヘッダの Space 転送（CU-05）は必須契約で回帰検査している。
  - 再生準備中の停止は投入時に音声側へ伝え、停止後は準備済み出力を再利用する（[ADR 0097](decisions/0097-cancellable-retained-transport.md)）。初回の素材・プラグイン初期化時間は残る。
  - 保持したプラグインの発音状態は停止・一時停止・位置移動で消去し、通常の余韻と区別する（[ADR 0099](decisions/0099-transport-plugin-state.md)）。
  - 音声グラフは音声側が排他所有し、構造更新中も現在の計画で再生を続けて準備完了後に交換する（[ADR 0102](decisions/0102-exclusive-audio-graph.md)）。
  - ライブ停止・一時停止は波形を 5 ms 以内に減衰させ（[ADR 0103](decisions/0103-live-stop-waveform.md)）、減衰後の後片付けはヘルパーの完了を待たずに返る（[ADR 0104](decisions/0104-stop-output-diagnostics.md)、[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)）。
- **音声バス・ループ・キャッシュ・描画**:
  - 素材・CLAP 間・書き出しのステレオ保持、配置音声のループ継ぎ目、5 ms のフェーダー補間と f64 加算を接続済み（[ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)）。暗黙のステレオ→モノ変換はエラーにする。
  - ループ先頭は全素材を事前準備して保持し、フェーダー前の出力元の差し替え口（`CachedTrack`）も接続済み（[ADR 0108](decisions/0108-phase1-loop-readiness.md)）。ディスクストリーミング、Phase 3 のレンダリングキャッシュ生成本体、Ogg、プラグインのサムネイルは未実装。
  - クリップゲインは `0..=1_048_576`（音声クリップのみ）。負の配置オフセットはファイル前の無音として扱い、内容位置 0 は `start - offset` のテンポ積分に置かれる（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
  - トラックメーターはフェーダー後の左右最大ピークであり、LUFS やトゥルーピークではない。
  - 表示リンクは画面の最大リフレッシュレートに追従し、波形バッファ再利用、ピークキャッシュの部分読出、再生・書き出し・ヘルパー処理中の App Nap 抑止を備える（[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)）。ペイン縮小時の描画クリップも接続済み（[ADR 0098](decisions/0098-pane-render-boundaries.md)）。
- **プラグイン隔離・Workgroup・キャッシュ**:
  - ヘルパーは `SIGABRT` / `SIGSEGV` / `SIGBUS` / `SIGILL` で共有領域にクラッシュ印を書く。AUv3 の二重隔離回避、プラグインウィンドウのフォーカス、スキャンの Seatbelt、Workgroup ポートの 2 ms ポーリング置換は未実装（[ADR 0111](decisions/0111-macos-helper-notices.md)）。
  - 選択デバイスの Workgroup をヘルパーの追加・保持・再起動へ引き継ぎ、変更通知で出力を再準備する。同一素材・同一配置結果の PCM 共有と、版・カメラによる可視列キャッシュを接続済み（[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)）。CPU 並列グラフ、メーカー単位のヘルパー集約、SME、Metal 4 は未実装。

## 検査基盤で区別すること

- **実 HAL と UI テストの TSan**: 2026-10-17 に UI テストが実 HAL を開いた際の TSan 警告は、UI テスト側を機器なし応答へ修正して分離したが、実 HAL バッファ再利用時の警告原因自体は未確定であり、抑制は行わない（[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)）。
- **errno TSan 警告**: Darwin arm64 のスレッド固有領域の再利用が、C だけの再現でも誤検出された。
  検査専用ランタイムで errno の寿命処理を修正し、同じ errno を実際に共有する競合の検出も保つ（[ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)）。
  正式な合否は検査レポートを参照する。
- **プラグイン発音アタック**: Vital Init の無音からの発音は 48 / 96 kHz・複数ブロック長でプラグイン単体出力と全サンプル一致しており、ホスト由来の不連続は再現していない（[ADR 0106](decisions/0106-natural-plugin-onsets.md)）。
- **検査種別の境界**: 通常の回帰検査、負の対照（`negative_control`）、オフスクリーン GPU 検査、実ウィンドウの `spike-playhead`、長時間計測（[ADR 0096](decisions/0096-measurement-completion.md)）、フックの直接検査と実ホスト発火、ローカル検査とリモート CI、ユーザーの聴感確認はそれぞれ別の証拠として区別する。
