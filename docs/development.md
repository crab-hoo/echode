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

JSON-RPC の外枠検証は `crates/echode-api/src/rpc.rs::ApiSession::reply` に置く。
公開 API・実ソケットの `tests/request_validation.rs` は、不正な ID・版・params の拒否と、
通知の応答省略・後続要求・ジャーナルと Undo / Redo・再オープンを検査する
（[ADR 0020 の後続修正](decisions/0020-json-rpc.md#要求の外枠の検証-2026-10-29)）。

JSON の構文上の空白は `ApiSession::reply` の `trim_matches` と既存の JSON パーサーで検証する。
`tests/request_whitespace.rs` は公開 API・実ソケットで不正な前後の空白を拒否し、ジャーナル・Undo / Redo・再オープンを保つ。
正しい空白・文字列内の Unicode と空白だけの行も検査する
（[ADR 0020 の空白検証](decisions/0020-json-rpc.md#json-の空白だけを除去する2026-10-29)）。

ソケットの改行完了は `serve.rs::serve_connection_limited` が検証する。
`tests/request_framing.rs` は EOF の部分行で編集・通知・Undo / Redo を実行しないことと、
完了済みの前方の編集・ジャーナル・既存の履歴・再オープン、CRLF と分割受信を検査する
（[ADR 0073 の改行境界](decisions/0073-socket-path.md#改行前の切断の拒否-2026-10-29)）。

MCP の改行完了は `crates/echode-api/src/mcp.rs::read_line` が stdio と接続先 API の応答の両方で検査する。
公開入口と実ソケットの `tests/mcp_framing.rs` は、部分行の拒否・前方の編集・履歴と再オープン、
版応答による編集の許可と編集応答の結果不明・再送禁止、LF / CRLF 込みの行上限を区別する
（[ADR 0115 の回帰補強](decisions/0115-phase2-mcp.md#改行境界の回帰補強2026-10-29)）。

MCP の接続先の error の形は `mcp.rs::exchange` で検証する。
`tests/mcp_responses.rs` は公開入口と実ソケットで、不正な応答の結果不明の案内・再送禁止と、
記録済み編集の Undo / Redo・再オープン、正当な error / result の保持を検査する
（[ADR 0115 の応答検証](decisions/0115-phase2-mcp.md#接続先のエラー応答の形の検証2026-10-29)）。

接続先 API の版確認は `mcp.rs::backend` に置く。
`tests/mcp_version.rs` は公開入口と実ソケットで、非対応・不正な版に単一／一括編集・Undo / Redo を送らないことと、
既存のジャーナル・Redo・再オープン、正当な版 1 での編集を検査する
（[ADR 0115 の版確認](decisions/0115-phase2-mcp.md#接続先の版確認の回帰補強2026-10-29)）。

MCP の接続先の JSON-RPC 版・ID・result/error 排他は、同じ `mcp.rs::exchange` で検証する。
`tests/support/mcp_envelopes.rs` の異常入力を `mcp_version.rs` と `mcp_responses.rs` で共有し、
編集前の未送信・既存 Redo と、編集後の結果不明・再送禁止・確定済み編集の保持を分けて検査する。
正当な追加フィールド・旧／新 MCP・単一 Undo / Redo・再オープンも通す
（[ADR 0115 の外枠回帰](decisions/0115-phase2-mcp.md#接続先の応答外枠の回帰補強2026-10-29)）。

MCP の要求 ID と通知は `McpSession::reply` が接続前に振り分ける。
`tests/mcp_requests.rs` は旧・新 MCP の単一／一括編集・Undo / Redo で不正 ID と通知を拒否し、
接続なし・ジャーナル・既存 Redo・再オープンと、正当な文字列／整数 ID の型と実編集を検査する。
実バックエンドは改行回帰と `tests/support/mcp_session.rs` を共有する
（[ADR 0115 の入力回帰](decisions/0115-phase2-mcp.md#要求-id-と通知の回帰補強2026-10-29)）。

旧 MCP の初期化と使用済み ID は `McpSession::legacy`・`reply` が接続前に検査する。
`tests/mcp_lifecycle.rs` は同じ stdio を保つ `support/mcp_client.rs` で、初期化前・不正な initialized・ID 再使用の拒否、
既存 Redo・確定した一括編集の単一 Undo / Redo・再オープンと、文字列／整数 ID の区別を検査する
（[ADR 0115 の初期化回帰](decisions/0115-phase2-mcp.md#旧-mcp-の初期化と-id-再使用の回帰補強2026-10-29)）。

プラグイン走査の期限・パイプ排出は `crates/echode-plugin-host/src/scan.rs`、回帰は `tests/scan.rs` に置く。
JSON・子孫のパイプ保持・ハング等のネイティブ検査専用バイナリは `tests/support/scan_helper.rs` とし、製品へ同梱しない
（[ADR 0190](decisions/0190-native-scan-deadline-fixtures.md)）。

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

古い保存版への復旧通知と書込み禁止は `DocumentHost::recovery_notice`・`accept_recovery`、
C ABI の受渡しは `queue.rs` の `historyReport.recovery` と `history.accept_recovery` に置く。
Swift の警告と開く要求の取消は `RecoveryConsent.swift`、公開前の確認は `ProjectController.open`、
画面の選択肢は `WorkspaceView.swift`、回帰は `recovery_tests.rs`・`RecoveryConsentTests.swift` に置く
（[ADR 0183](decisions/0183-recovery-consent.md)）。

値の操作子のクリック規則と重ねる入力欄は `ValueControl.swift`（`ValueClicks`・`ValueEntryField`）、
単位と範囲は `MixControls.swift`（`MixEntry`・`ClipEntry`）、各操作子の接続は `ArrangeHeaderView.swift`・`ArrangeView.swift`・
`WorkspaceParameterView.swift`、回帰は `ValueControlTests.swift` に置く。
リセット待ちから別の操作子へ移る対象境界は `ValueControlTargetTests.swift` に置く。
ヘッダの別フィールド・別トラックと、クリップの別ハンドルの数値欄を製品入力から検査する
（[ADR 0153 の回帰補強](decisions/0153-value-control-clicks-and-typed-entry.md#回帰の補強-リセット待ちから別の操作子へ移る)）。
クリップ側の文書境界は `ArrangeView` の `audioDocument`・`ClipPress`・`gestureEdit` に置く。
`ClipGestureDocumentTests` は再オープン時の押下・下書き・数値欄の取消、投入済み送信の再照合と押下時の編集先保持を検査する
（[ADR 0194](decisions/0194-clip-mix-gesture-document-scope.md)）。
プラグインの文字の変換は、通信の `ParseParam`（`crates/echode-sandbox/src/params.rs`）、ヘルパーの `param_from_text`、
FFI の `device.parameter.parse`、画面の `enterParameter` を通す（[ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md)）。

選択トラックのミュート・ソロは `MixerSwitchBatch.swift` の対象捕捉・順序待ち列・アクション配列を通す。
`WorkspaceView` と `MixInspectorView` は同じ `arrangeMixActions` を使い、ピアノ行の単一対象と区別する。
回帰は `MixerSwitchBatchTests.swift`・`MixerSwitchBoundaryTests.swift`・`MixerSwitchValidationTests.swift`
（[ADR 0189](decisions/0189-selected-track-mute-and-solo.md)）。

音量・パンの数値入力の対象・共通倍率／差分・上下限は `MixerValueBatch.swift`、入力の振分けは `MixActions.enter` を使う。
回帰は `MixerValueBatchTests.swift`・`MixerValueBoundaryTests.swift`・`MixerValueOrderTests.swift` に置く。
離散的な入力は `mixerSwitchTail` を共有し、既存の単一フェーダーの終了後に手番の現行値を使う
（[ADR 0191](decisions/0191-selected-track-numeric-mix.md)）。
ヘッダの読み上げ増減は `MixActions.adjust`・`captureMixerStep` と `MixerValueBatch.direction` を通す。
回帰は `MixerStepTests.swift`・`MixerStepBoundaryTests.swift`・`MixerStepOrderTests.swift` に置く
（[ADR 0192](decisions/0192-selected-track-accessibility-steps.md)）。

ヘッダのマウス操作の文書境界は `ArrangeHeaderView` の `documentID`・`press`・`gestureActions` に置く。
回帰は `MixerGestureDocumentTests.swift`。文書交換で押下・リセット待ち・数値欄を取り消し、同文書の更新では押下時のコールバックを保持する
（[ADR 0193](decisions/0193-header-mix-gesture-document-scope.md)）。
横移動ゼロの音量保持は `MixScale.dragged` の恒等分岐を通す。
回帰は `MixerDragIdentityTests.swift`。縦ドラッグと解放時の原点復帰で、表示下限未満の値・Undo / Redo・保存を保持する。

クリップの直交ドラッグと解放時の原点復帰は `ClipDragIdentityTests.swift` で検査する。
非正規化数・端数 flick・長いフェード、Redo と実パッケージの再オープンを通す。
保存の浮動小数点表記は `document.rs::format_gain`、独立した保存回帰は履歴の `tests/float_roundtrip.rs` に置く
（[ADR 0196](decisions/0196-lossless-package-floats.md)）。同じ回帰で `DocumentHost` の増分保存・コミットからの再オープン・履歴復帰と Undo / Redo も通す。

保存済み音声の有限性は `crates/echode-history/src/media.rs::decode_pcm`、公開読込みとトリム後の素材準備の回帰は
`crates/echode-audio-io/tests/pcm_validation.rs` に置く（[ADR 0056 の後続修正](decisions/0056-audio-import.md)）。

外部音声の取込み前の有限性は `media.rs::decode_file`、公開復号・取込みと履歴の回帰は
`crates/echode-history/tests/audio_import_validation.rs` に置く。
IEEE float WAV の異常値・有限値のビット保持・原本とパッケージの保持・一括編集の巻戻しと再オープンを通す。

保管庫とキャッシュの Git 除外は `objects.rs::ensure_store_gitignore`、回帰は履歴の `tests/store_gitignore.rs` に置く。
公開取込みとホスト開始で、コメント・似たパスの誤認拒否、CRLF・既存テキストの保持と二重実行を通す
（[ADR 0031 の後続修正](decisions/0031-content-addressed-store.md#除外規則の行単位の照合-2026-10-29)）。

波形キャッシュの構造・バイト長・列の検証は `peaks.rs::read_cache`、公開読込みと再生成の回帰は
`crates/echode-history/tests/peak_cache_validation.rs` に置く（[ADR 0058](decisions/0058-waveform-peaks.md)）。
有限ステレオの平均とキャッシュの版は `stereo_waveform.rs` で検査する。
構築・マップの極大値と非正規化数、旧キャッシュの再生成と PCM の保持を製品の公開波形経路へ通す。
境界の途中をまたぐ列の無音は `waveform_silence.rs` で検査する。
素材前・EOF 後・配置外の包絡、粗い列とマップの端・テンポ変化とキャッシュ再利用を公開経路へ通す。

クリップの移動と複製移動のモデルは `crates/echode-model/src/clip_move.rs`（置き換えの `land` と導出 ID）、
画面のドラッグ・影・要求（`ClipMove`）と controller の編集は `ArrangeClipMove.swift`、押下の振り分けは `ArrangeView.swift` に置く。
回帰は `clip_move_tests.rs`、履歴と CLI の `clip_move`、`ClipMoveTests.swift`（[ADR 0154](decisions/0154-clip-move-and-duplicate.md)）。
単一クリップの ⌘D は `ArrangeView` のローカルなキー入力と `ClipMoveTarget.adjacentStart` を通す。
回帰は `ClipDuplicationTests.swift`・`ClipDuplicationBoundaryTests.swift`
（[ADR 0188](decisions/0188-arrange-adjacent-clip-duplication.md)）。
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

自動／固定グリッドは `PianoGrid.swift`（音価と実効幅・ネイティブ操作子）と `PianoCamera.gridMode` に置く。
`PianoGridView.setGridMode` が押下の取消と所有確認、`PianoFrame.gridTicks` が表示幅内の縦線を扱う。
回帰は `PianoGridTests.swift`・`PianoGridControlTests.swift`、GPU の細分線は `PianoOffscreenProbe.swift` に置く
（[ADR 0185](decisions/0185-piano-auto-and-fixed-grid.md)）。
三連符・付点の候補と表示名も `PianoGrid.swift` に置き、実操作子・入力と履歴は `PianoRhythmicGridTests.swift`、
全音価と表示幅の境界は `PianoRhythmicGridBoundaryTests.swift` で検査する
（[ADR 0186](decisions/0186-piano-triplet-and-dotted-grids.md)）。
描画の時間目印は `PianoFrame.gridLineTicks`、通常回帰は `PianoTimeGuideTests.swift` に置く。
吸着由来の `gridTicks` と分け、GPU の独立した目印の採取数は `checkedIndependentGuides` へ記録する
（[ADR 0187](decisions/0187-piano-independent-time-guides.md)）。

空白の左ドラッグ追加は `PianoInsertion.swift` の押下時の開始・カメラと解放時の幾何を使い、
`PianoGridView` の `mouseUp` から既存の挿入へ送る。仮表示は `PianoRenderInput.insertion` と `PianoFrame`、
回帰は `PianoInsertionTests.swift` に置く（[ADR 0171](decisions/0171-piano-blank-drag-insertion.md)）。

単クリックの記憶長入力も同じ `PianoInsertion` を使い、長さと非永続の履歴枠は `NoteInputLength.swift` に置く。
`ProjectController.recordEdit` が全モデル編集の枠をそろえ、ノート選択・追加・長さ変更だけが長さを更新する。
回帰は `NoteInputLengthTests.swift`・`NoteInputHistoryTests.swift`（[ADR 0172](decisions/0172-piano-click-and-input-length.md)）。

ベロシティの専用モデルは `crates/echode-model/src/note_velocity.rs`、製品アクションは `note.velocity`、
画面の相対ドラッグと共通入力値は `NoteVelocity.swift`、仮表示は `PianoFrame.swift` に置く。
回帰は `NoteVelocityTests.swift`・履歴の `note_velocity.rs` と `block_size.rs`
（[ADR 0173](decisions/0173-piano-note-velocity.md)）。
下部の棒の共通幾何とヒットは `PianoVelocityLane.swift`、遅延生成するスライダーは `PianoVelocityAccessibility.swift`、
回帰は `NoteVelocityLaneTests.swift` と `PianoOffscreenProbe.swift` に置く
（[ADR 0175](decisions/0175-piano-velocity-lane.md)）。
ピアノロールの共通カメラは `CanvasNavigationState.pianoCamera`、復元と文書の所有確認は `PianoGridView` に置く。
切替・再生成・旧文書の拒否の回帰は `PianoViewportTests.swift`（[ADR 0174](decisions/0174-shared-piano-viewport.md)）。

複数選択の集合は `ProjectController.noteSelection`、右ドラッグの交差判定は `PianoSelection.swift` に置く。
入力・右メニュー・取消は `PianoGridView`、一括削除は `ProjectController.deleteNotes` の単一トランザクションを通す。
回帰は `PianoSelectionTests.swift`・`PianoSelectionDeletionTests.swift`
（[ADR 0176](decisions/0176-piano-note-selection-and-deletion.md)）。
一括ベロシティの対象検証とアクション生成は `NoteVelocityBatch.swift`、共通差分と仮表示は `NoteVelocity.swift` に置く。
回帰は `NoteVelocityBatchTests.swift`・`NoteVelocityBatchBoundaryTests.swift`
（[ADR 0177](decisions/0177-piano-batch-note-velocity.md)）。
一括移動の対象検証と共通差分・仮表示は `PianoMoveBatch.swift`、確定は `ProjectController.moveNotes` に置く。
回帰は `PianoMoveBatchTests.swift`・`PianoMoveBatchBoundaryTests.swift`・`PianoMoveBatchCancellationTests.swift`
（[ADR 0178](decisions/0178-piano-batch-note-movement.md)）。

Command 複製移動は `PianoMoveGesture.copies` と `PianoMoveChange.actions`、成功後の選択は `ProjectController.moveNotes` に置く。
共有内容の複製は `Project::duplicate_note` と `note.duplicate`、回帰は `PianoDuplicate*Tests.swift` と履歴の `note_duplicate.rs` に置く
（[ADR 0184](decisions/0184-piano-note-duplicate-drag.md)）。

一括伸縮の対象検証・共通の端の差分と仮表示は `PianoResizeBatch.swift`、確定は `ProjectController.resizeNotes` に置く。
回帰は `PianoResizeBatchTests.swift`・`PianoResizeBatchBoundaryTests.swift`・`PianoResizeBatchCancellationTests.swift`
（[ADR 0179](decisions/0179-piano-batch-note-resizing.md)）。

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
