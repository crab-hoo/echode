# エージェントの開発手順

必須規則は [AGENTS.md](../AGENTS.md)。この文書は実際の作業手順とコードの所在を案内する。
検査基盤は Python 3.11 以降（標準の `tomllib` を使用、追加パッケージ不要）で動く。
動的検査の初回は `make setup-bridge` を使う。TSan の固定ソース取得・CMake / Ninja と検査専用ランタイムは [検査手順](verification.md#環境フックと-ci) を参照する。

## 開始から完了まで

1. `git status --short` で既存変更を把握し、他の作業の変更を巻き戻さない。
   `make agent-context` で不足ツール・鮮度・選択された検査・関連資料を一度に確認する。
   個別確認は `make doctor` と `make verify-plan`、必要な節の読込と根拠の扱いは [エージェントの手順](agent-workflow.md) に従う。
2. [現在の制約](status.md)、触る領域の [ARCHITECTURE](ARCHITECTURE.md) 分割資料、関連 ADR、[監査の読み順](README.md#監査の読み順) を読む。
   設計上の将来機能と実装済みの機能を区別し、下記の保守の観点から今回の影響範囲も確認する。
3. 不具合は最小入力、初期状態、操作順、期待と実際、使用した版を記録する。
   製品の入口から再現し、回帰テストを先に失敗させる。環境エラーは [デバッグ手順](debugging.md) で切り分ける。
4. 境界と不変条件を守って修正し、必要なドキュメント・テスト・デバッグ基盤も同じ作業で更新する。
   作業中の個別テストは `-q` を付けて出力を抑え（例: `cargo test -q -p <crate> <filter>`）、最後の編集後に `make verify` を実行する。
5. 選択された実機・計測は実行するか理由を記録し、最後の編集後に全選択項目を確認する。途中の `--only` の成功を完了の証拠にしない。
6. 最終報告には変更の効果、基盤の更新内容（不要なら理由）、失敗した検査、見送った検査名と理由、残る制約を簡潔に記す。
   検査の報告範囲は [AGENTS.md](../AGENTS.md#進捗と結果の報告) に従う。
   コミット・プッシュはユーザーの依頼がある時だけ行う。

実機・聴感テストは原則としてユーザーが担当し、エージェントは実機を使わない自動回帰を先に行う。
音声だけの変更に Computer Use を使わず、機器を開く検査やアプリ操作は明示的な依頼がある時だけ行う。
前の作業の変更が残っている時も、各検査の依存入力が同じなら合格を再利用する。
10分計測（`spike-engine`）は大きなリアルタイム設計変更・長時間不具合・リリース確認など、目的を説明できる時に追加する。

## 検査をまとめて進める

検査の範囲・合否基準・必須契約を維持し、重複実行とエージェントへ渡す出力を抑える。

- 関連する実装・資料・回帰テストを一つの完了条件にまとめ、更新をそろえてから最終検査する。
- 作業中は再現・修正確認に必要な個別テストを `-q` で実行し、最後の編集後に `make verify` で全選択項目をまとめて処理する。
  再実行は追加変更・失敗の修正・未解決の懸念がある時に行う。
- 結果の再利用と失効は [検査基盤](verification.md#記録と有効期限) に任せ、入力とツールが同じ有効な結果を使う。
- 検査を `grep` / `head` へ直接パイプしたり、`;` や最後の `echo` で失敗コードを隠したりしない。
  独立した複数の個別検査は [安全なコマンド実行](verification.md#作業中の個別コマンド) で全終了コードを集約する。
  意図した red も期待終了コードと回帰固有のアサーションまで照合し、コンパイル失敗や時間切れで代用しない。
- 詳細ログは検査基盤の保存先に残し、成功時は集計、失敗時は抜粋と必要な範囲だけを読む。
  実行中は完了をまとめて待ち、短い間隔の進捗確認や成功ログの読み直しを避ける。
  ユーザーへの報告は [AGENTS.md](../AGENTS.md#進捗と結果の報告) に従う。
- 一括実行でも検査の排他と必須契約の負の対照ごとの隔離を維持する。
  反例は 1 件ずつ入れ、ビルドを並列にしてもテストは他の作業がない状態で 1 件ずつ実行する（[ADR 0148](decisions/0148-verification-pipeline-speed.md)）。
  検査同士は並列にしない。負荷でテストの期限が外れると、失敗が反例の検出と区別できない。
  実機・計測の実行条件と未実施理由の記録は [検査手順](verification.md) に従う。

## 基盤の保守を同じ作業で行う

開始時に今回の変更と下記の関係を確認し、作業中に分かった事実と完了時の実装に合わせて見直す。
変更がない資料や基盤を形式的に書き換えず、必要な保守を別の依頼へ残した状態では完了にしない。

### ドキュメント

- 利用方法・コマンド・設定が変われば `README.md` と該当手順を更新する。
- 実装範囲・制約が変われば [status.md](status.md)、設計が変われば [ARCHITECTURE.md](ARCHITECTURE.md) と対応する [docs/architecture/](architecture/) 分割資料および ADR、方針・完了条件が変われば [PLAN.md](PLAN.md) を更新する。
- どの資料が正本かは [資料の入口](README.md) に従い、同じ事実を複数の資料へ重複追記しない。
- 行指定読込でのトークン消費を抑えるため、1 文または 1 項目ごとに改行し、1 行を概ね 250 バイト以下に保つ（[ADR 0119](decisions/0119-agent-token-efficiency.md)）。
- 監査の指摘を直したら、当時の失敗記録は保持したまま、対応と回帰テストへの参照を追記する。

### テスト基盤

- 振る舞いや境界を追加・変更したら、通常・異常・境界の入力を既存のテストが通せるか確認する。
- 新しいアクションには `crates/echode-history/tests/properties.rs`、音を作る処理には `crates/echode-engine/tests/block_size.rs` の回帰を追加する。
- 製品ソースファイル内の単体テストが大きくなった場合（目安 100 行以上）は、`#[cfg(test)] #[path = "<module>_tests.rs"] mod tests;` で同ディレクトリの別ファイルへ分離し、製品コードの閲覧コストを抑える。
- モジュール・依存・ツールチェーン・実行入口が変わったら、`scripts/verification.json`、`Makefile`、CI、実行手順を照合する。選択条件は `scripts/verification.json` だけに置く。
- 必須契約（`product_contracts`）は、正しいソースで実テストが 1 件通過し、誤りを戻したソースのコピーでは期待値で失敗することを確認する。

### デバッグ基盤

- 新しい失敗経路・環境依存・ツールの取得エラーが判明したら、[デバッグ手順](debugging.md) に識別方法と調査先を反映する。
- ログ・成果物の出力先や形式、新しいヘルパー・バンドル・外部入力を変えたら、`make doctor` の鮮度照合と `make debug-report` の収集範囲を見直す。
- 古い結果・環境不足・製品不良を区別できること、秘密情報やユーザー文書を収集しないこと、音声スレッドへ禁止操作を持ち込まないことを維持する。

### 完了時の確認

実装、説明、検査が保証する範囲、失敗時に集められる証拠が一致することを確認する。
最終報告では今回の基盤の更新内容（または更新不要の理由）を簡潔に示す。

## コードの所在

| 領域 | パス | 役割・主な入口 |
| :--- | :--- | :--- |
| モデル・時間・不変条件 | `crates/echode-model` | `model.rs`（`Project`）、`time.rs`（`Tick` / `Flicks` / `TempoMap`）、`invariants.rs`、`changeset.rs`、`duplicate.rs`（部分木の複製と導出 ID） |
| アクション・編集セッション | `crates/echode-actions` | `action.rs`（`Action` / 登録簿）、`session.rs`（`Session` の Undo / Redo 履歴） |
| Git 履歴・パッケージ・ピーク | `crates/echode-history` | `host.rs`（`DocumentHost`・ジャーナル・復旧）、`store.rs`（CAS）、`peaks.rs`（ピークキャッシュ） |
| リアルタイム検査 | `crates/echode-realtime` | `alloc.rs`（RT 確保禁止ガード）、`event.rs`（固定容量イベント列）、`ring.rs`（SPSC） |
| オーディオエンジン | `crates/echode-engine` | `playback.rs`（再生器・`PlaybackAudio`・`CachedTrack`）、`plugins.rs`（別プロセス CLAP ステージ）、`bypass.rs`（ドライ平面の通過）、`mix.rs`（フェーダー補間・f64 加算）、`declick.rs`（停止減衰）、`render.rs`（オフライン書出し）、`device.rs`（実機計測） |
| CoreAudio I/O・素材読込 | `crates/echode-audio-io` | `stream.rs`（HAL 入出力）、`opening.rs`（出力準備の別スレッド・機器なしの検査用出力）、`session.rs`（文書の音声セッション）、`clips.rs`（WAV 読込・SRC・PCM 共有キャッシュ）、`transport.rs`（取消世代）、`workgroup.rs` / `hardware.rs`（Workgroup・機器通知）、`trace.rs`（停止出力診断）、`activity.rs`（スリープ抑止） |
| 共有メモリ・IPC | `crates/echode-sandbox` | `region.rs`（共有メモリ・`arm_abort_notice`）、`session.rs`（`PluginControl`）、`workgroup.rs`、`wire.rs` |
| プラグインヘルパー | `crates/echode-plugin-host` | `scan.rs`（バンドル走査）、`catalog.rs`（キャッシュ）、`threads.rs` / `events.rs`（CLAP 処理・リセット）、`awake.rs` |
| 同梱音源 | `crates/echode-sine-pad` | Sine Pad CLAP、事前領域拡張、状態と余韻。`scripts/build-bundled-plugins.sh` で通常の走査用バンドルを作る |
| 故障注入プラグイン | `crates/echode-test-plugin` | クラッシュ・ハング・ステレオ・残響・MIDI 検証用 CLAP プラグイン |
| 描画クエリ・可視列 | `crates/echode-draw` | `query.rs`（アレンジ・ピアノロールの可視列抽出・空間インデックス） |
| JSON-RPC・MCP | `crates/echode-api` | `rpc.rs` / `serve.rs`（Unix ソケット RPC）、`mcp.rs`（MCP stdio ブリッジ） |
| CLI | `crates/echode-cli` | `render`、`serve`、`mcp` サブコマンド。統合テストは `tests/render.rs`、`tests/mcp.rs` |
| 製品 C ABI | `crates/echode-ffi` | `lib.rs`、`queue.rs`（コマンド投入・取消）、`controls.rs`。ヘッダ生成は `cbindgen` |
| Swift ブリッジ | `swift/EchodeBridge` | `Document.swift`、`Audio.swift`、`Controls.swift`（`echode-ffi` の Swift 6 ラッパー） |
| 製品 macOS アプリ | `swift/EchodeApp` | `Sources/EchodeAppCore/`：`ProjectController.swift`、`Arrange*.swift`、`Piano*.swift`、`Mix*.swift`、`DeviceChain.swift`、`Workspace*.swift`、`Canvas*.swift` |
| スパイク（検証用） | `spikes/` | `draw`（S1）、`bridge`（UI-2）、`input`（S5）。スパイクの型を製品へ流用しない |
| 検査・診断・配布スクリプト | `scripts/` | `verify.py`、`verification.json`、`check_product_contracts.py`、`diagnose.py`、`distribution.py`、`tests/` |

フォルダ表示の製品 ABI は `crates/echode-ffi/src/projection.rs`、Swift の保持と階層取得は
`swift/EchodeBridge/Sources/EchodeBridge/TrackHierarchy.swift` に置く。
ヘッダの開閉と表示行の共有は `ArrangePaneView.swift`、追加位置の保持は `TrackInsertion.swift` を使う。
複製対象の保持は `TrackDuplication.swift`、ヘッダの右メニューは `ArrangeHeaderView.swift` に置く。
CLAP パラメーターの通信は `crates/echode-sandbox/src/params.rs`、ヘルパー側は `threads.rs` の `flush_params` / `param_infos`、
FFI は `crates/echode-ffi/src/device.rs`（検査・往復・記録）と `lane.rs`（ワーカー外のデバイスレーン）に置く。
画面は `DeviceParameters.swift`（デバイスごとの状態・世代・ピン留め）、`WorkspaceParameterView.swift`（画面内の行だけの一覧）、
`WorkspaceDeviceView.swift`（横並びの列とドロップ）に置く。
列の開閉と文書 ID の検査は `DeviceDisclosure.swift`、幅・入力・フォーカスの制御は `WorkspaceDeviceView.swift` に置く。
ヘッダの FX 名前と残りのチェインのメニューは `TrackFXNames.swift`、入力とアクセシビリティは `ArrangeHeaderView.swift` に置く。
テスト用 CLAP のパラメーターは `crates/echode-test-plugin/src/params.rs` が持つ。
プラグイン独自のウィンドウは、ヘルパーの `crates/echode-plugin-host/src/gui.rs`（窓と規則）・`gui_appkit.rs`（AppKit とキー入力）と `relay.rs`（出力イベントのリング）、
通信の `crates/echode-sandbox/src/gui.rs`、FFI の `crates/echode-ffi/src/activity.rs`（文書単位の受け口）に置く。
画面は `PluginWindows.swift`（開く要求・活動の読取り・記録）、テスト用 CLAP の窓は `crates/echode-test-plugin/src/gui.rs` が持つ。
サンプルの試聴は、ボイスを `crates/echode-engine/src/audition.rs`、試聴スレッドと専用 IOProc を `crates/echode-audio-io/src/audition.rs`、
FFI の入口を `echode_document_audition`、画面の選択の扱いと要求の記録（`AuditionLog`）を `SampleAudition.swift`、
ブラウザの一覧の入力を `WorkspaceFileList.swift` に置く（[ADR 0143](decisions/0143-browser-click-and-audition-timing.md)）。
試聴の部分読込みは `crates/echode-history/src/media.rs` の `AudioFileReader`、連続 SRC は
`crates/echode-engine/src/resample_stream.rs`、先読みと波形の集約は `crates/echode-audio-io/src/audition_load.rs` に置く。
ブラウザの部分波形は `WorkspaceSamplePreview.swift`、機器なしの開始計測は `make spike-audition` を使う。
試聴の設定・手動要求は `SampleAudition.swift`、実操作子は `WorkspaceAuditionControls.swift`、
表示寿命の通知は `WorkspaceView.swift`、元サンプル位置の seek は `AudioFileReader` に置く（[ADR 0152](decisions/0152-browser-audition-controls.md)）。
最初の出力準備は `crates/echode-audio-io/src/opening.rs`（`Outputs`・`Opening`・`HeadlessOutput`）と `session.rs` の `install`、
準備を待つデバイス要求の順序は `crates/echode-ffi/src/queue.rs` の `waiting` と `device.rs` の `Waiting` に置く（[ADR 0136](decisions/0136-output-opening-off-worker.md)）。
開いた出力の置換グラフの準備スレッドと回収は `crates/echode-engine/src/playback_preparation.rs`、
要求の集約と公開は `playback.rs` の `publish_pending` に置く（[ADR 0142](decisions/0142-background-plugin-graph-preparation.md)）。
停止中の置換の素材読込みスレッドと `MediaInputs` は `crates/echode-engine/src/playback_media.rs`、
再生の保留と `audio.prepare` は `crates/echode-audio-io/src/session.rs`、開いた時の送信は `ProjectController.preparesOutput` に置く
（[ADR 0147](decisions/0147-output-prewarm-and-background-media.md)）。

値の操作子のクリック規則と重ねる入力欄は `ValueControl.swift`（`ValueClicks`・`ValueEntryField`）、
単位と範囲は `MixControls.swift`（`MixEntry`・`ClipEntry`）、各操作子の接続は `ArrangeHeaderView.swift`・`ArrangeView.swift`・
`WorkspaceParameterView.swift`、回帰は `ValueControlTests.swift` に置く。
プラグインの文字の変換は、通信の `ParseParam`（`crates/echode-sandbox/src/params.rs`）、ヘルパーの `param_from_text`、
FFI の `device.parameter.parse`、画面の `enterParameter` を通す（[ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md)）。

クリップの移動と複製移動のモデルは `crates/echode-model/src/clip_move.rs`（置き換えの `land` と導出 ID）、
画面のドラッグ・影・要求（`ClipMove`）と controller の編集は `ArrangeClipMove.swift`、押下の振り分けは `ArrangeView.swift` に置く。
回帰は `clip_move_tests.rs`、履歴と CLI の `clip_move`、`ClipMoveTests.swift`（[ADR 0154](decisions/0154-clip-move-and-duplicate.md)）。
自動配置の中身の小節と移動後の合わせ直しは `placement.rs::refit_one` と `invariants.rs`、
旧ジャーナルの種類の互換と保存は `crates/echode-history/tests/clip_bar_basis.rs` で検査する
（[ADR 0170](decisions/0170-content-relative-auto-bars.md)）。

記憶した再生開始位置と停止での復帰は、セッションの `transport.set_start` と停止（`crates/echode-audio-io/src/session.rs`）、
余韻を切らないエンジンの `PlaybackCommand::Rewind`（`playback.rs`）、パッケージの `transport.json`（`crates/echode-history/src/transport.rs`）に置く。
画面はピアノロールのルーラーのクリックと印（`PianoRollView.swift`・`PianoFrame.swift`）、Space と一時停止の振り分け（`ProjectController.swift`）、
回帰は `PlaybackStartTests.swift` と `stopped_processing.rs`（[ADR 0155](decisions/0155-remembered-playback-start.md)）。

ルーラーの範囲とドラッグ状態は `RulerLoop.swift`（`RulerPress`）に置く。
アレンジは `ArrangeRuler.swift`（入力・帯・印・数字）と `ArrangeFrame.swift`・`ArrangePainter.swift`（共通上端とクリップ）、
ピアノロールの仮表示と確定は `PianoRollView.swift`、帯は `PianoFrame.swift` に置く。
アレンジの回帰は `ArrangeRulerTests.swift`（[ADR 0157](decisions/0157-arrange-ruler.md)）。
既存端のグリップの共通幾何と描画は `RulerHandles.swift`、片端だけの調整は `RulerPress.resize`、
入力・取消と範囲の共有の回帰は `RulerHandleTests.swift` に置く（[ADR 0158](decisions/0158-ruler-loop-edge-handles.md)）。
再生ヘッドへの追従の判定と主スレッドの表示リンクは `PlayheadFollow.swift`（`PlayheadFollow`・`PlayheadFollower`）、
ページ送りと手動スクロールでの解除は `ArrangePaneView.swift` と `PianoRollView.swift`、値は `WorkspaceState.followsPlayhead` に置く。
回帰は `PlayheadFollowTests.swift`（[ADR 0159](decisions/0159-playhead-follow.md)）。

単一ノートのドラッグの吸着・Shift と最短長は `PianoGeometry.swift` の `PianoGesture.preview`、
入力・解放時の計算と単一送信は `PianoRollView.swift` の `notePreview`・`mouseUp` に置く。
回帰は `PianoDragTests.swift`（[ADR 0167](decisions/0167-piano-note-drag-snapping.md)）。

空白の左ドラッグ追加は `PianoInsertion.swift` の押下時の開始・カメラと解放時の幾何を使い、
`PianoGridView` の `mouseUp` から既存の挿入へ送る。仮表示は `PianoRenderInput.insertion` と `PianoFrame`、
回帰は `PianoInsertionTests.swift` に置く（[ADR 0171](decisions/0171-piano-blank-drag-insertion.md)）。

単クリックの記憶長入力も同じ `PianoInsertion` を使い、長さと非永続の履歴枠は `NoteInputLength.swift` に置く。
`ProjectController.recordEdit` が全モデル編集の枠をそろえ、ノート選択・追加・長さ変更だけが長さを更新する。
回帰は `NoteInputLengthTests.swift`・`NoteInputHistoryTests.swift`（[ADR 0172](decisions/0172-piano-click-and-input-length.md)）。

アレンジの音高表示の音域と縦変換は `crates/echode-draw/src/query.rs` の `PitchRange`・`PitchLayout` に置く。
製品フレームと VoiceOver 用矩形の回帰は `ArrangePitchTests.swift`、可視列は `tests/arrange_pitch.rs` を使う。
問い合わせだけの計測は `make spike-arrange-query`（[ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
製品と検証用のループへの進入判定は `PlaybackAudio::arm_loop` と `Transport::arm_loop`、回帰は `RulerLoopTests.swift` と
`block_size.rs` の `contract_loops_enter_from_before_and_bypass_from_the_end_under_rtsan`（[ADR 0156](decisions/0156-ruler-loop-range-and-entry.md)）。

描画面の左上固定と drawable サイズの主スレッド更新は `CanvasSurface.swift`、対応する製品 View の回帰は `CanvasResizeTests.swift` に置く
（[ADR 0144](decisions/0144-unscaled-canvas-resizing.md)）。

下部の寸法と積み方は `WorkspaceLayout.swift` と製品の `WorkspaceCenterStack.swift`、
開閉・移行とコマンドは `WorkspaceState.swift`・`WorkspaceCommand.swift` に置く。
回帰は `WorkspaceLayoutTests.swift`（[ADR 0146](decisions/0146-independent-editor-stack.md)）。

音声のドラッグ元は `WorkspaceFileList.swift`、ファイル URL の位置指定と要求保持は `ArrangeAudioDrop.swift`、
AppKit の受け口は `ArrangeView.swift`、トランザクション内の取込み解決は `crates/echode-history/src/host.rs` に置く
（[ADR 0145](decisions/0145-sample-browser-file-drops.md)）。

### ビルド対象の CPU・OS 基準

- 対象は macOS 27.0 以降、Apple M4 ファミリー以降（[ADR 0112](decisions/0112-m4-minimum.md)、[ADR 0114](decisions/0114-m4-build-baseline.md)）。
- Rust と C / C++ 依存は `.cargo/config.toml`、Swift Package は各 `Package.swift`（`-target-cpu apple-m4` と `-Xcc -mcpu=apple-m4` を対で指定）、Xcode は `project.yml`、直接コンパイルする補助コードは `scripts/build-platform.env` に揃える。
- 実行ホストの確認は `scripts/check_build_host.py`。

## 回帰テストを選ぶ基準

内部関数の戻り値だけでなく、失敗した製品の境界を通す。
再生位置なら HAL 登録前の準備経路、プラグインなら実バンドルの別プロセス走査と処理、キー入力なら `NSView.keyDown` から上位への転送を確認する。

- **ブロック長独立性**: 音を変える処理には `crates/echode-engine/tests/block_size.rs` の比較を追加し、位置・チャンネル・長さの境界値も確認する。
- **モデル不変条件と履歴**: アクションを追加・変更したら `crates/echode-history/tests/properties.rs` の生成へ追加し、`Project::check_invariants`、Undo / Redo、保存・再読込を通す。
- **保存・復旧**: 成功応答の後に再オープンして確認する。I/O 失敗、途切れた記録、二重書き手も関係範囲で扱う。
- **Swift UI**: 実 View の入力、controller の操作、モデルへの反映を混同しない。
- **必須契約の登録**: `scripts/verification.json` の `product_contracts` に ID・実行経路・目的・誤りを戻す差分（`negative_control`）を登録する。コンパイル失敗は検出の証拠にならない。
