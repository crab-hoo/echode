# デバッグ手順

目的は、製品の不具合、古いアプリ、検査の不足、取得ツールや実行環境の問題を区別し、次のエージェントが同じ入力で再現できるようにすること。

## 最初に確認すること

```sh
make doctor
make verify-plan
make debug-report
```

- `doctor` はソースを変更せず、アプリ・ウィンドウ・音声デバイス・プラグインを開かない。
  現在のソース、必要なツール、検査結果の有効期限（`current` / `partial` / `stale`）、ロックの保持、開発用アプリ（`target/Echode.app`）の出所を表示する（機械向けは `python3 -B scripts/diagnose.py --json`）。
- `debug-report` は `target/debug-reports/<日時>-<一意名>/` へ診断 JSON、案内、既存ログ（末尾最大 2 MiB）とスパイクの JSON をコピーする。
  生の環境変数・Cargo 設定・ユーザープロジェクトは収集しない。
- `make app` は署名後の本体・ヘルパーのハッシュとソースを `target/Echode.build.json` に記録する。
- ソースが同じでも実機計測だけが古くなったら、既定出力とレートの変更、`system_profiler -json SPAudioDataType -detailLevel mini` の読取可否を確認する。機器設定を診断のために勝手に変えない。

## 症状から調べる順番

### ビルド・環境・検査基盤

- **空き容量不足（`No space left on device` / `Errno 28`）**:
  `df -h` で空き容量を確認する。検査のロック解除後に、再生成可能なコンパイラーキャッシュや一時データだけを整理し、既存の検査結果・ユーザー資料・開いているアプリは削除しない。空き確保後に `make doctor` と `make verify` を再実行する。
- **ツール不足・ビルド失敗**:
  `doctor` の `missing_tools` と最初のエラーを読む。通常ツールは `make setup`、動的検査は `make setup-bridge`。
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
- **CLAP が鳴らない・走査失敗**:
  プラグイン ID、バンドル走査（macOS の `.clap` ディレクトリを含む）、キャッシュ、ヘルパー起動、入出力バスを追う。走査失敗時は子の終了コードとシグナルを確認する。本体プロセスにプラグインをロードして切り分けてはいけない。
- **空白への移動・再開で以前の残響が鳴る**:
  停止・シーク時の `active.reset()`、`NOTE_CHOKE`、宣言された全 MIDI ポートへの CC120 / All Notes Off を確認し、自然な余韻と区別する（[ADR 0099](decisions/0099-transport-plugin-state.md)）。
- **左右の消失・定位変化・ループやフェーダーの段差**:
  逆位相・異振幅の左右信号で素材読込・エフェクト間・フェーダー後を照合する（`stereo-media`、`stereo-plugin-chain`、`audio-loop-seam`、`audio-fader-ramp`、[ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)）。
- **ループ変更での再読込・トリム位置ずれ**:
  `clips.rs` の保持キーと `CachedTrack` の失効条件を確認する（[ADR 0108](decisions/0108-phase1-loop-readiness.md)）。内容位置 0 は `start - offset` のテンポ積分で、負のオフセットは無音となる（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
- **長時間・短時間の実機音声計測の失敗**:
  処理期限超過（`late`）と HAL 呼出欠落（`gaps`、`pmset -g log` の Sleep / Wake や `/usr/bin/log show` の coreaudiod デバイス変更）を区別する（[ADR 0096](decisions/0096-measurement-completion.md)）。短時間確認は `--native-rate` を使う。

### UI・描画・入力・履歴・MCP

- **直したのに画面が変わらない**:
  `doctor` でアプリが `stale` / `unknown` なら `make app` を実行し、`target/Echode.app` を開き直す。
- **FX が違うトラックへ入る・並びが変わらない**:
  候補を開いた文書セッション、追加操作時の選択トラック、ドロップ先トラックを区別し、`device.reorder` / `device.copy` と状態ハッシュの引き継ぎを `fx-chain-ui` で確認する（[ADR 0118](decisions/0118-track-fx-chain-editing.md)）。
- **画面のミックス値・メーターと音が一致しない**:
  確定スナップショットと一時的な `faderPreview`、`requested_plan` / `applied_plan` を確認する。
  メーターはフェーダー後の左右最大ピーク（停止中は 24 dB/s 減衰、−60 dB 以下で 0、無音表示は `−∞`）。VoiceOver の増減段数や数値欄の `−` 解釈は `MixControlTests` で確認する（[ADR 0107](decisions/0107-mix-controls.md)）。
- **プレイヘッド・リフレッシュレート・ペイン境界・行境界**:
  プレイヘッドは音声が最後に書いた tick を補間せず描画する。`displayFrameRateRange`（最大 120 Hz）、ペイン縮小時の `clipsToBounds`（[ADR 0098](decisions/0098-pane-render-boundaries.md)）、オフスクリーン検査の左余白比較点（`x=2 pt`）を確認する（[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)）。
- **キー入力・ホイール・検索・ループ欄**:
  ヘッダの `keyDown` から上位への Space 転送、マウスホイールの行単位とトラックパッドのポイント単位（[ADR 0050](decisions/0050-arrange-viewport.md)）、ピアノロールのドラッグ中のみの Escape と連続矢印キー（[ADR 0052](decisions/0052-piano-keyboard.md)）、空選択時の検索解除（[ADR 0053](decisions/0053-workspace-ui-foundation.md)）、ループ欄の確定時送信（[ADR 0055](decisions/0055-loop-metronome.md)）を確認する。
- **保存・復旧・Undo / Redo・MCP**:
  バックグラウンドのプラグイン状態保存は Undo に積まず Redo も消さない（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
  MCP（`echode mcp SOCKET`）は接続先の `echode serve PACKAGE SOCKET` と同じビルド・絶対ソケットパス・API 版 1 を確認し、stdout へログを混ぜない（[ADR 0115](decisions/0115-phase2-mcp.md)）。

### サニタイザー（TSan / ASan / RTSan）

- **TSan の既知の環境要因と製品不具合の切り分け**:
  1. UI テストは機器なしの応答差し替え口を使い、実 HAL を開かない（`target/macos27-audit/tsan-hal-before.log`）。
  2. Rust 標準ライブラリの終了済みスレッドと新規スレッドの `errno` 競合は、製品を含まない最小 staticlib + Swift でも再現している（`target/m4-build-audit/`、[ADR 0114](decisions/0114-m4-build-baseline.md)）。
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
