# デバッグ手順

目的は、製品の不具合、古いアプリ、検査の不足、取得ツールや実行環境の問題を区別し、次のエージェントが同じ入力で再現できるようにすること。

## 最初に確認すること

```sh
make agent-context
make debug-report
```

- `doctor` はソースを変更せず、アプリ・ウィンドウ・音声デバイス・プラグインを開かない。
  現在のソース、必要なツール、検査結果の有効期限（`current` / `partial` / `stale`）、ロックの保持、開発用アプリ（`target/Echode.app`）の出所を表示する（機械向けは `python3 -B scripts/diagnose.py --json`）。
- `agent-context` は同じ診断経路と検査選択を一度で使い、要約と根拠の JSON を作る。
  取得の失敗、本文予算の超過、曖昧な節、古い行番号の識別と復旧は [エージェントの手順](agent-workflow.md) を参照する。
- `debug-report` は `target/debug-reports/<日時>-<一意名>/` へ診断 JSON、案内、既存ログ（末尾最大 2 MiB）とスパイクの JSON をコピーする。
  生の環境変数・Cargo 設定・ユーザープロジェクトは収集しない。
- `make app` は署名後の本体・ヘルパー・同梱プラグインのハッシュとソースを `target/Echode.build.json` に記録する。
- ソースが同じでも実機計測だけが古くなったら、既定出力とレートの変更、`system_profiler -json SPAudioDataType -detailLevel mini` の読取可否を確認する。機器設定を診断のために勝手に変えない。

## 症状から調べる順番

### ビルド・環境・検査基盤

- **空き容量不足（`No space left on device` / `Errno 28`）**:
  `df -h` で空き容量を確認する。検査のロック解除後に、再生成可能なコンパイラーキャッシュや一時データだけを整理し、既存の検査結果・ユーザー資料・開いているアプリは削除しない。空き確保後に `make doctor` と `make verify` を再実行する。
- **ツール不足・ビルド失敗**:
  `doctor` の `missing_tools` と最初のエラーを読む。通常ツールは `make setup`、動的検査は `make setup-bridge`。
  `swift test -q` が `Build failed` / `fatalError` だけを返す場合は、同じ引数へ `-v` を加えて `target/` のログへ保存し、`error:` 周辺を読む。
  Xcode 側のビルド診断が quiet 出力に出ない場合がある。大量のコンパイルコマンドは会話へ展開しない。
- **Rust に追加したアクションを Swift が `unknown variant` として拒否する**:
  Swift のテストは Rust の製品 ABI をリンクするため、先に `make check-shell` で ABI と生成ヘッダを更新して再リンクする。
  Swift の単体実行だけでは古いアクション登録簿が残り得る。更新後も失敗する場合は、送信した `type` と登録簿を照合する。
- **macOS 27 / M4 ビルド基準の不一致**:
  Cargo 生成物は `xcrun vtool -show-build <バイナリ>` と `lipo -archs` で確認する（単体の `rustc --print deployment-target` は `.cargo/config.toml` を読まない）。
  Swift は `-target-cpu apple-m4` と `-Xcc -mcpu=apple-m4` を対で指定する（[ADR 0114](decisions/0114-m4-build-baseline.md)）。
  release の proc-macro / cdylib 読込失敗（`E0463` / `mis-aligned LINKEDIT string pool`）は `.cargo/config.toml` の `strip = "none"` を確認する（[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)）。
  `scripts/check_build_host.py` が `sysctl machdep.cpu.brand_string` を読めない隔離環境では、CPU 情報の読取が許される環境で再実行する。
- **実行環境の権限制限（共有メモリ・キーチェーン）**:
  `shm_open: Operation not permitted` や `SecKeychainCreate` の失敗はサンドボックス制限によるため、許可された環境で再実行する。
- **自動検査・製品契約の失敗**:
  `make verify` 失敗時は標準エラーに出力される失敗箇所の抜粋を確認し、必要なら `target/verification/<ID>.log` または `product-contracts/` の該当ログを読む。
  最適化等で `negative_control` が期待値で落ちなくなった場合は、本来の不具合を再現する差分へ更新する（例: `loop-head-residency`）。
  一時コピー削除時の `.DS_Store` 競合は自動リトライされるが、権限エラー等の継続失敗は合格にしない。
- **基準コミット（`VERIFY_BASE`）エラー**:
  一時リポジトリのテストでは親の `VERIFY_BASE` を隔離する。無効な ref 指定は HEAD へ黙って置換せずエラーにする。
- **終了フック直後の不要な再実行**:
  ツール識別は PATH 文字列ではなく解決された `rustc` / `swift` 等の実パスを使う。

### オーディオ・トランスポート・プラグイン

- **S2 / S3 は通るのに製品だけ負荷に弱い**:
  選択デバイスの Workgroup 取得・ヘルパー参加・変更通知後の再準備を確認する（[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)）。`plugin_missed_blocks` と実機条件を記録する。
- **再生開始が遅い・準備中に停止できない**:
  `pending-transport`（UI 受付）、`queued-stop`（ABI 投入時取消）、`transport-cancellation`（HAL 初回発音禁止）の 3 境界を切り分ける（[ADR 0097](decisions/0097-cancellable-retained-transport.md)）。
- **再生中のぶつぶつ・停止時のクリック**:
  `audio.status` の `plugin_missed_blocks` と `pending_plan` を確認する。
  定期保存確認による一時バイパス排除（[ADR 0101](decisions/0101-live-output-discontinuities.md)）、音声側の排他グラフ所有（[ADR 0102](decisions/0102-exclusive-audio-graph.md)）、5 ms の停止波形減衰（[ADR 0103](decisions/0103-live-stop-waveform.md)）、待機しない後片付け（[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)）のどの境界かを確認する。
- **音源なしのノート・Sine Pad が鳴らない**:
  音源のないトラックは無音である。デバイス一覧に `dev.echode.sine-pad` があるかを確認する。
  一覧にない時は `make bundled-plugins` とヘルパー隣の `plugins/`、アプリ内の `Contents/PlugIns/`、同梱用走査キャッシュを確認する。
  `host event capacity was not prepared` はイベント領域の準備漏れであり、音声側の確保や固定発音数で回避しない
  （[ADR 0123](decisions/0123-explicit-sine-pad-plugin.md)）。
- **同梱 CLAP の初回だけ RTSan が malloc を検出する**:
  子プロセスの標準エラーを収集し、`_tlv_get_addr` と `ForbidAlloc` が並ぶかを確認する。
  dylib 固有の検査用 TLS は、ヘルパーの最初の非 RT 起動段階の `start_processing` で初期化する。
  検知を外さず、未発音の新しいヘルパーで最初の処理も検査する。
- **CLAP が鳴らない・走査失敗**:
  プラグイン ID、バンドル走査（macOS の `.clap` ディレクトリを含む）、キャッシュ、ヘルパー起動、入出力バスを追う。走査失敗時は子の終了コードとシグナルを確認する。本体プロセスにプラグインをロードして切り分けてはいけない。
- **バイパスの表示と音が一致しない・解除後に古い音が鳴る**:
  文書のデバイス ID と `bypassed`、計画の要求・適用版、プラグイン失敗、ドライ／ウェットの平面数を確認する。
  バイパスした音源ではノートが無音でも同じトラックの配置音声は鳴る。解除時のリセットと現在のノート追跡を確認する（[ADR 0120](decisions/0120-device-bypass.md)）。
- **オフラインの FX が読み込まれる・素材まで無音になる・書出しに失敗する**:
  文書の `offline`、`bypassed`、`state` を分け、保存形式 5 と旧版からの変換を確認する。
  `fx-offline-ui` は実ボタンと履歴、`plugin-offline` は隔離グラフと記録済み状態の復元、`offline-export` は実 CLI のログである。
  `chain_of` と `project_plugin_ids` / `routes_with_saved_state` の対象、計画の要求・適用版を確認する。
  `device rendering requires the isolated plugin host` が全オフライン時に出たら、ホストなし書出しの対象判定を確認する。
  再有効化後の未検出・状態オブジェクトの読込エラーとは区別する（[ADR 0125](decisions/0125-device-offline.md)）。
- **空白への移動・再開で以前の残響が鳴る**:
  停止・シーク時の `active.reset()`、`NOTE_CHOKE`、宣言された全 MIDI ポートへの CC120 / All Notes Off を確認し、自然な余韻と区別する（[ADR 0099](decisions/0099-transport-plugin-state.md)）。
- **左右の消失・定位変化・ループやフェーダーの段差**:
  逆位相・異振幅の左右信号で素材読込・エフェクト間・フェーダー後を照合する（`stereo-media`、`stereo-plugin-chain`、`audio-loop-seam`、`audio-fader-ramp`、[ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)）。
- **ループ変更での再読込・トリム位置ずれ**:
  `clips.rs` の保持キーと `CachedTrack` の失効条件を確認する（[ADR 0108](decisions/0108-phase1-loop-readiness.md)）。内容位置 0 は `start - offset` のテンポ積分で、負のオフセットは無音となる（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
- **長時間・短時間の実機音声計測の失敗**:
  処理期限超過（`late`）と HAL 呼出欠落（`gaps`、`pmset -g log` の Sleep / Wake や `/usr/bin/log show` の coreaudiod デバイス変更）を区別する（[ADR 0096](decisions/0096-measurement-completion.md)）。短時間確認は `--native-rate` を使う。

### UI・描画・入力・履歴・MCP

- **起動直後から新規作成・開くが無効で「変更を保存中」のまま**:
  前回プロジェクトの自動復元と `ProjectController.busy` を確認する。
  `sample` の読取で復元側が `Autosave::drop` の join、`echode-autosave` が初期化失敗後の受信待ちなら、終了要求を無視する旧不具合と一致する。
  初期化失敗中も `Shutdown` で終了し、復元エラーとパッケージロックの解放を返す必要がある。
  `startup-recovery` は子プロセスの停止防止期限でこの循環待ちを検出し、Swift の `failedStartupOpenReenablesNewAndOpen` はエラー後の新規作成を確認する。
  復元対象の削除や前回パスの初期化で問題を隠さず、エラーと Git の状態を切り分ける。
- **直したのに画面が変わらない**:
  `doctor` でアプリが `stale` / `unknown` なら `make app` を実行し、`target/Echode.app` を開き直す。
- **FX が違うトラックへ入る・並びが変わらない**:
  候補を開いた文書セッション、追加操作時の選択トラック、ドロップ先トラックを区別し、`device.reorder` / `device.copy` と状態ハッシュの引き継ぎを `fx-chain-ui` で確認する（[ADR 0118](decisions/0118-track-fx-chain-editing.md)）。
- **トラックが違う位置へ追加される・追加後の選択が違う**:
  `TrackInsertionRequest` の文書セッションと `after`、ジャーナルの `track.add` / `track.insert` と `into_folder` を区別する。
  展開フォルダは最後の子、折りたたみ中は部分木の外側、末尾ボタンは最上位末尾へ追加する。
  `TrackInsertionTests` と `track_insertion` のログで操作後の選択変更・消えた対象・旧文書の拒否を確認する
  （[ADR 0127](decisions/0127-positioned-track-insertion.md)）。
- **複製が違う位置に入る・二重に増える・元のノートまで変わる**:
  `TrackDuplicationRequest` の文書セッションと `sources`（選択された祖先を持つ行は除く）、ジャーナルの `track.duplicate` の `source` と `seed` を区別する。
  複製の ID は `seed` から導出した UUIDv8 で、元と同じ中身 ID の配置が複製先に残っていれば独立化の不具合である。
  `TrackDuplicationTests` と履歴・CLI の `track_duplication` で、位置・選択・中身の独立・書出しの 2 倍一致を確認する
  （[ADR 0129](decisions/0129-track-duplication.md)）。
- **フォルダ開閉後に行・ミックス値・FX の対象がずれる**:
  元スナップショットと表示用ハンドルの行数・ID、`TrackTreeInfo` と `row_map` を区別する。
  ヘッダ、可視矩形、ミキサーとデバイスが同じ表示行から元トラックを参照することを
  `FolderDisclosureTests` と `snapshot_ownership` の必須契約で確認する。
  開閉後に古いフレームが反映される場合は、入力ハンドルと描画キャッシュの版も確認する
  （[ADR 0128](decisions/0128-folder-disclosure-and-row-projection.md)）。
- **FX の一括追加が一部だけ・Undo が複数回になる**:
  単一フォーカスと `selectedTracks`、クリック時の `PluginInsertionRequest` の文書・対象配列を区別する。
  製品 ABI が配列を同じ `DocumentHost::apply` へ渡すことを `TransactionTests` と `fx-batch-ui` のログで確認する。
  選択変更後の付け替え、旧文書、欠けた対象、途中失敗時の部分追加を分けて調べる（[ADR 0124](decisions/0124-selected-track-fx-batches.md)）。
- **サンプル検索で見つからない・配置先や位置が違う**:
  登録フォルダの一覧と索引の読込中表示、読込失敗の詳細を確認し、素材変更後は更新ボタンで再読込する。
  索引は隠しファイル・シンボリックリンク・パッケージ内部を含めず、重なる登録から同じパスを重複表示しない。
  配置は `AudioImportRequest` の文書セッション・トラック・tick を使う。
  定期更新の `audio.tick` とライブの `playback.position.tick` を区別し、非同期待機後の選択へ付け替えない。
  `SampleBrowserTests` と必須契約の検索・配置ログを確認する（[ADR 0122](decisions/0122-sample-library-search-and-placement.md)）。
- **画面のミックス値・メーターと音が一致しない**:
  確定スナップショットと一時的な `faderPreview`、`requested_plan` / `applied_plan` を確認する。
  メーターはフェーダー後の左右最大ピーク（停止中は 24 dB/s 減衰、−60 dB 以下で 0、無音表示は `−∞`）。VoiceOver の増減段数や数値欄の `−` 解釈は `MixControlTests` で確認する（[ADR 0107](decisions/0107-mix-controls.md)）。
- **プレイヘッド・リフレッシュレート・ペイン境界・行境界**:
  プレイヘッドは音声が最後に書いた tick を補間せず描画する。`displayFrameRateRange`（最大 120 Hz）、ペイン縮小時の `clipsToBounds`（[ADR 0098](decisions/0098-pane-render-boundaries.md)）、オフスクリーン検査の左余白比較点（`x=2 pt`）を確認する（[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)）。
- **キー入力・ホイール・検索・ループ欄**:
  ヘッダの `keyDown` から上位への Space 転送、マウスホイールの行単位とトラックパッドのポイント単位（[ADR 0050](decisions/0050-arrange-viewport.md)）、ピアノロールのドラッグ中のみの Escape と連続矢印キー（[ADR 0052](decisions/0052-piano-keyboard.md)）、空選択時の検索解除（[ADR 0053](decisions/0053-workspace-ui-foundation.md)）、ループ欄の確定時送信（[ADR 0055](decisions/0055-loop-metronome.md)）を確認する。
- **保存・復旧・Undo / Redo・MCP**:
  バックグラウンドのプラグイン状態保存は Undo に積まず Redo も消さない（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
  CLI 書出しは `DocumentHost` の復旧スナップショットを使う。
  テストで既存パッケージの JSON だけを上書きしても保存済み履歴は置き換わらない。文書の編集経路を使うか、別の入力は新しいパッケージへ書く。
  MCP（`echode mcp SOCKET`）は接続先の `echode serve PACKAGE SOCKET` と同じビルド・絶対ソケットパス・API 版 1 を確認し、stdout へログを混ぜない（[ADR 0115](decisions/0115-phase2-mcp.md)）。

### サニタイザー（TSan / ASan / RTSan）

- **TSan の既知の環境要因と製品不具合の切り分け**:
  1. UI テストは機器なしの応答差し替え口を使い、実 HAL を開かない（`target/macos27-audit/tsan-hal-before.log`）。
  2. 終了済みと新規のスレッドの `errno` 誤検出は、C のみでも再現し、検査ランタイムの寿命処理を修正した（[ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)）。
     `target/bridge/thread/errno/` の baseline・fixed・real-race の対照と、同じランタイムのロード証拠を確認する。
     準備が古い時は `make setup-bridge` を使う。取得済みソースの再ビルドには `python3 scripts/tsan_runtime.py --prepare` も使える。
  いずれも抑制（suppression）や不要なロックで隠さず、スタックと対象アドレスを確認して製品データの競合と区別する。

## 再現と引き継ぎに残す情報

### 停止クリック・発音アタックの実出力診断

- `ECHODE_STOP_TRACE_DIR` を指定した診断実行では、停止から 50 ms 後までの HAL 実出力を `.f32le` と `.json` に記録する（通常ビルドは記録無効、[ADR 0104](decisions/0104-stop-output-diagnostics.md)）。
  `stop_frame - first_frame`、位相の連続性、`stop_fade_offset`、`cleanup_pending`（ライブ後片付けの非待機保留、[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)）、`render_ns` / `elapsed_ns` と `output_host_time` の呼出間隔を照合する。
  ユーザーの再生音が含まれるため、通常の `debug-report` には自動収集しない。
- 無音からの発音アタックは、別プロセスでの直接出力と製品の再生・出力補正後の全サンプルを比較し、音源本来のアタックとホストの追加変化を区別する（[ADR 0106](decisions/0106-natural-plugin-onsets.md)）。

### 共通の記録項目

1. 対象のコミットと作業ツリー、診断資料の場所、実行ファイルの出所。
2. 最小の入力と初期状態、操作順、期待と実際（文書・トラック・ノート等の ID を含む）。
3. 実行した正式な検査名、終了コード、期待値が実際に走った証拠、ログの場所。
4. 原因の根拠と未確定の部分（製品不良・テスト起動・ツール取得・環境条件の区別）。
5. 修正した境界と回帰テスト、未実行の検査・理由、残る制約。

`/tmp` だけの再現や画像を唯一の修正根拠にせず、通常のテストか登録プローブへ移して監査に参照を残す。
