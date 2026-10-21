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
  `swift test -q` が `Build failed` / `fatalError` だけを返す場合は、`swift build --build-tests` で診断を取得する。
  同じ `--package-path`・`--scratch-path` と `-Xswiftc -warnings-as-errors` を指定し、`target/` のログで `error:` 周辺を読む。
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
  Swift のマニフェスト読込時に `ModuleCache: Operation not permitted` と標準ライブラリ読込失敗が続く場合も、先にキャッシュの書込権限を確認する。
  文書だけの検査でもビルド設定と一時署名の検査を含むため、これらの環境エラーを製品の型エラーや設計資料の不整合と混同しない。
- **自動検査・製品契約の失敗**:
  `make verify` 失敗時は標準エラーに出力される失敗箇所の抜粋を確認し、必要なら `target/verification/<ID>.log` または `product-contracts/` の該当ログを読む。
  最適化等で `negative_control` が期待値で落ちなくなった場合は、本来の不具合を再現する差分へ更新する（例: `loop-head-residency`）。
  一時コピー削除時の `.DS_Store` 競合は自動リトライされるが、権限エラー等の継続失敗は合格にしない。
  単独では通る保存・復旧の回帰が、復旧後のトラック数などの過剰で落ちる時は、`$TMPDIR` に残った同じ pid の一時パッケージを疑う。
  失敗した過去の実行が残したジャーナルが再生され、2026-10-21 に `phase1-acceptance` で再現した。
  一時パッケージを作る `scratch()` は使用前に同じパスを削除する。
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
  [ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md) 以降は素材をプラグインの手前で減衰し、マスターの停止フェードは使わない。
  `stop_fade_offset` は素材の減衰位置である。停止後も余韻・自発的な発音が出ることは正常で、停止中の `plugin_missed_blocks` も実際の処理失敗を表す。
- **停止中にプラグインの時間・鍵盤の発音が止まる、または余韻が切れる**:
  `playing = false` とライブ処理の停止を混同しない。保持した出力は無音入力の処理を続ける。
  `stopped-plugin-output` で実ヘルパーと後段 FX の出力、`stopped-plugin-tails` でノートオフと自然な余韻の終端を確認する。
  音源・FX の `bypassed` / `offline`、出力が既に開いているか、位置移動・準備取消でリセットが要求されていないかを分ける。
  停止だけで `cleanup_pending` が立つなら旧仕様か不具合を疑う。位置移動の後片付け中なら正常である。
  位置移動直後の停止で期限切れになる場合は `stop-cleanup` を使い、保留中に通常の素材処理を始めていないか確認する。
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
- **パラメーター一覧が出ない・値が音に反映しない・Undo で音が戻らない**:
  デバイスの列の表示文が境界を示す。`the project has no running plugin` と `the device has no running plugin` は文書・オフライン・未検出、
  `no output device is available` は停止中の出力起動、`parameter changes timed out` はヘルパーの処理スレッドの `flush` を疑う。
  `document thread stopped` は文書を閉じた後の要求、`device request panicked` はデバイスレーンでの異常終了である。
  一覧取得の失敗は列の「パラメーターを再読込み」で対象だけを再試行する。検索・選択・履歴は変えず、再失敗も同じ列に出す。
  プラグインの再起動や状態の再読込とは異なるため、クラッシュしたヘルパーや未検出のプラグインの復旧を保証しない。
  再試行で他の列も消える・連打で要求が重なる場合は `device-list-retry-ui` と `DeviceParameterRetryTests` を使う
  （[ADR 0140](decisions/0140-device-parameter-list-retry.md)）。
  `plugin-params` は実ヘルパーとエンジン、`fx-params-ui` は画面と履歴のログである。
  Undo では記録のハッシュとヘルパーが最後に保存・読込したバイトを比べ、一致すれば読み込まない（[ADR 0130](decisions/0130-clap-parameter-editing.md)）。
- **空白への移動・再開で以前の残響が鳴る**:
  停止・シーク時の `active.reset()`、`NOTE_CHOKE`、宣言された全 MIDI ポートへの CC120 / All Notes Off を確認し、自然な余韻と区別する（[ADR 0099](decisions/0099-transport-plugin-state.md)）。
  ライブ停止では余韻を残す。上の消去は位置移動・未発音の準備取消とオフライン停止の検査であり、ライブ停止へ戻さない
  （[ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
- **左右の消失・定位変化・ループやフェーダーの段差**:
  逆位相・異振幅の左右信号で素材読込・エフェクト間・フェーダー後を照合する（`stereo-media`、`stereo-plugin-chain`、`audio-loop-seam`、`audio-fader-ramp`、[ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)）。
- **ループ変更での再読込・トリム位置ずれ**:
  `clips.rs` の保持キーと `CachedTrack` の失効条件を確認する（[ADR 0108](decisions/0108-phase1-loop-readiness.md)）。内容位置 0 は `start - offset` のテンポ積分で、負のオフセットは無音となる（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
- **長時間・短時間の実機音声計測の失敗**:
  処理期限超過（`late`）と HAL 呼出欠落（`gaps`、`pmset -g log` の Sleep / Wake や `/usr/bin/log show` の coreaudiod デバイス変更）を区別する（[ADR 0096](decisions/0096-measurement-completion.md)）。短時間確認は `--native-rate` を使う。

### UI・描画・入力・履歴・MCP

- **ヘッダの名前が出ない・名前で候補が開く・違う窓が開く**:
  名前は行高 52 pt 以上かつ幅と字下げに余裕がある時に表示する。先頭以外は「…」のメニューで選ぶ。
  `TrackFXNamesLayout` の矩形とカタログ名、スナップショットのチェイン順を確認する。
  名前は `onOpenPlugin`、追加ボタンは `onOpenFX` であり、`fx-header-ui` で入力と古い対象の拒否を区別する。
  その後のヘルパーの表示失敗は既存の `plugin-window-helper` の経路を調べる（[ADR 0139](decisions/0139-track-header-fx-names.md)）。
- **窓アイコンが消えない・閉じた窓が再び開として表示される**:
  アイコンは物理的な可視性ではなく寿命を示す。別アプリの操作中に隠れていても開の表示は正しい。
  `plugin-window-state` でソケット受信順の `window_state`、`plugin-window-status-ui` で画面の `openPluginWindows` を確認する。
  活動の `window` は編集時の過去の文脈、`closed` は未読期間内の閉鎖の有無であり、現在の表示へ代用しない。
  開く要求の返答から楽観的に表示を更新していないか、未読圧縮で最後の寿命を保っているかも調べる。
  削除・オフライン後の残留は `publish`、文書交換後の残留は `stopPluginActivity` と文書 ID の照合を確認する
  （[ADR 0138](decisions/0138-plugin-window-status.md)）。
- **デバイスの一覧が隠れる・折りたたみ後の FX ドロップ先がずれる**:
  名前の左の開閉状態を確認する。折りたたみは音やオフライン状態を変えず、検索・一覧・状態の本文だけを隠す。
  展開しても一覧が戻らない時はパラメーターの取得失敗と区別する。
  `device-disclosure-ui` で可変幅の列の中央前後と送り先 ID を確認し、配置・挿入先に固定の列幅を使っていないか調べる。
  トラック切替・エディタ再生成で開閉が戻るなら `collapsedDevices`、文書交換後も残るなら `forgetParameters` を確認する。
  見えない検索欄がキーを消費する時は折りたたみ時のフォーカス解除を確認する（[ADR 0137](decisions/0137-device-disclosure.md)）。
- **パラメーター・バイパス・オフライン操作で全ボタンが一瞬暗転する**:
  `ProjectController.busy` の完了時の値だけでは短い暗転を見落とす。`device-edit-enabled-ui` で操作中の全遷移を観測する。
  `EditTurn.busy` は編集待ち行列の状態、画面の `busy` は作成・交換・終了の明示的な排他だけである。
  排他の既定値を通常編集へ広げていないか、各ビューが個別の `historyBusy` と全体の `busy` を取り違えていないか確認する。
  出力を開いた後の FX 追加で新しいヘルパーの起動を待つ制約とは別の経路である（[ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
- **プラグイン付きトラックの選択が止まる・パラメーター画面が重い**:
  2026-10-20 のユーザー報告は「選択のたび」と「汎用パラメーター画面」の 2 症状であり、独自プラグイン GUI の問題ではない。
  機器なしで、旧欄の全行一括生成（800 個で約 0.68 秒）と再選択での作り直し、文書ワーカー上の応答待ちを再現し、対応した
  （[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）。
  再発時は、プラグイン名・版・パラメータ数、再生状態、下部の表示、初回と再選択、停止時間、入力全体が固まるか操作が無効表示になるかを区別する。
  初回だけ一覧が遅いなら、音声状態の `opening` を見る。真の間に編集・Undo が終わるなら、出力の準備（ヘルパー起動・素材）の時間である。
  `opening` の間に編集も終わらないなら、ワーカーが準備を待っている。`output-opening-worker` を実行して手番を確かめる。
  導入後に位置・ループ・追加したトラックが反映されないなら `output-opening` で導入時の差分を確かめる（[ADR 0136](decisions/0136-output-opening-off-worker.md)）。
  FX の追加・オフライン解除で止まるなら、開いた出力への `replace` がワーカー上で新しいヘルパーを起動する待ちである（残る制約）。
  毎回止まるなら、`DeviceParameterListView.visibleRows` の数と `table.numberOfRows` を比べ、全行を作っていないかを確認する。
  列が作り直されているなら `DeviceStripView` の列の保持（文書の切替でだけ捨てる）を確認する。
  値の往復で止まるなら、`device-lane-held` を実行し、背景保存の確認と適用済み状態の読取りが通信中の制御端点を待っていないか確かめる。
  画面だけが古い一覧へ戻るなら、`device-view-slow-ui` で要求世代の照合を確かめる。
  テスト用 CLAP の `ECHODE_TEST_FAULT=params-wait:<socket>` は一覧をメインスレッドで保留し、機器なしで遅いプラグインを再現する。
  実測時間を使う性能検査は通常回帰から分ける。実機操作とサンプリングが必要な場合は担当・許可を確認し、既存アプリの終了・置換は行わない。
  [追加受入条件](phase2.md#p2-02-デバイスビューとパラメーター編集着手中)を参照する。
- **プラグイン GUI がすぐ消える・ループ切替で状態が戻る・Space が届かない**:
  `plugin-window-visibility` は OS の一時非表示を閉鎖と誤判定する経路を確認する。
  `NSPanel.close` の記録、親・自分・同じ親のヘルパーの最前面判定を調べ、実際の閉鎖と隠れただけの状態を区別する。
  `plugin-window-loop` は状態の取り込み後に未保存の値を変え、停止中・再生中のループ切替で同じ GUI と値が残ることを確認する。
  `Pending.restore_states` がループだけで真にならないこと、保留中のプロジェクト編集の復元を落とさないことを調べる。
  キーは `plugin-key-events`（AppKit）、`plugin-key-socket`（制御通知）、`plugin-key-ui`（既存コマンド）で境界を分ける。
  文字入力中は転送しない。独自入力欄は `NSTextInputClient.selectedRange` の公開状態を確認する。
  実画面の再発時はプラグイン名・版、前面のアプリ、窓を開く前の状態保存、入力欄の編集中かも記録する
  （[ADR 0134](decisions/0134-plugin-window-lifetime-and-shortcuts.md)）。
- **プラグインのウィンドウが開かない・フォーカスを奪う・値や Undo が届かない**:
  エラー表示で境界を分ける。`the plugin has no window` はプラグインが `clap.gui` を持たない、
  `the plugin has no Cocoa window` は Cocoa の API を持たない、`the plugin refused the window` は `set_parent` の拒否である。
  `plugin window timed out` はヘルパーのメインスレッドが 8 秒以内に `create` / `show` を終えなかったことを示す（[ADR 0132](decisions/0132-helper-plugin-windows.md)）。
  開くまでの待ちは、停止中の最初の表示で出力とヘルパーを準備する待ち（`opening`）と区別する。
  ウィンドウがキーになる・隠れない・戻らない場合は、記録用の窓で規則を確かめる `plugin-window-helper` を先に実行し、
  合格なら AppKit の実画面の問題としてユーザーの確認へ回す。実画面の操作は依頼がある時だけ行う。
  値が一覧へ届かないなら `plugin-window-relay` で処理スレッドのリングと制御スレッドの読み戻しを、
  Undo が増えない・増えすぎるなら `plugin-window-undo-ui` でジェスチャーの数え方と静止の待ちを確かめる。
  窓が閉じている時のプラグイン自身の変化は自動保存だけで、Undo に入らないのが正しい。
  テスト用 CLAP の `ECHODE_TEST_FAULT=gui:<socket>` は、ヘルパーに記録用の窓を選ばせ、ソケットからプラグイン画面の操作を送る。
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
- **サンプルの試聴が鳴らない・古い素材が鳴る・止まらない**:
  スナップショットの音声状態 `audition` の `request`・`handled`・`phase`・`error`・`sample_rate` を見る。
  `handled` が `request` に届かなければ試聴スレッドが処理前、`failed` は復号か出力準備の失敗、`sample_rate` が空なら出力は閉じている。
  機器の通知を受けると出力を閉じ、`stopped` と機器変更のエラーを残す。
  ボイスは `echode-engine` の試聴の単体回帰、スレッドとコールバックは必須契約 `sample-audition` で切り分ける
  （[ADR 0135](decisions/0135-sample-audition.md)）。
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
