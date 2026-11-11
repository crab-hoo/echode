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
   コミットは `make verify` が通った作業ごとに行い、プッシュは依頼がある時だけ行う（[ADR 0276](decisions/0276-priority-order-and-delegated-decisions.md)）。

実機・聴感テストは原則としてユーザーが担当し、エージェントは実機を使わない自動回帰を先に行う。
音声だけの変更に Computer Use を使わず、機器を開く検査やアプリ操作は明示的な依頼がある時だけ行う。
前の作業の変更が残っている時も、各検査の依存入力が同じなら合格を再利用する。
10分計測（`spike-engine`）は大きなリアルタイム設計変更・長時間不具合・リリース確認など、目的を説明できる時に追加する。

## 検査をまとめて進める

検査の範囲・合否基準・必須契約を維持し、重複実行とエージェントへ渡す出力を抑える。

- 関連する実装・資料・回帰テストを一つの完了条件にまとめ、更新をそろえてから最終検査する。
- 作業中は再現・修正確認に必要な個別テストを `-q` で実行し、最後の編集後に `make verify` で全選択項目をまとめて処理する。
  再実行は追加変更・失敗の修正・未解決の懸念がある時に行う。
  サニタイザと全反例を含む全体の段階は GitHub の `full` が実行し、作業ごとには実行しない（[検査手順](verification.md#二つの段階)）。
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
- 実画面・VoiceOver・聴感・実機の確認待ちを作ったら [ユーザー確認の一覧](user-checks.md) に項目を足し、status.md からはその番号を示す。
- 実装範囲・制約が変われば [status.md](status.md)、設計が変われば [ARCHITECTURE.md](ARCHITECTURE.md) と対応する [docs/architecture/](architecture/) 分割資料、方針・完了条件が変われば [PLAN.md](PLAN.md) を更新する。
- status.md には利用者に残る制約と未確認の項目だけを書く。制約が消えたら status.md の項目も消す。
- ADR を新しく書くのは、次のどれかに当たる判断だけである。
  - 他の案と比べて選んだ。選ばなかった案と理由を書ける。
  - 後から変えにくい。保存形式、公開する API・ABI、プロセスの境界、依存ライブラリ、ビルド基準など。
  - 複数の領域や、全エージェントの作業方法にかかわる方針である（検査、CI、資料の規則）。
  - 既存の ADR の判断を変える。または AGENTS.md・PLAN.md が ADR を求める（自作の理由と測定値など）。
- 既存の設計の範囲で機能を足す、不具合を直す、テストを足す時は ADR を作らない。
  今の挙動と設計は領域別の設計書へ、変更の理由と証拠（直す前に失敗した回帰など）はコミットメッセージへ書く。
  監査の指摘を直した時は、従来どおり監査の文書にも対応を書く。
  2026-11-05 までは実装の経緯も ADR にしており、30 日で 269 件になった。既存の ADR はそのまま残す。
- 資料の大きさは [読む量の上限](agent-workflow.md#読む量の上限) に収める。超えた時は上限を上げず、経緯を移すか見出しで節を分ける。
- どの資料が正本かは [資料の入口](README.md) に従い、同じ事実を複数の資料へ重複追記しない。
- 行指定読込でのトークン消費を抑えるため、1 文または 1 項目ごとに改行し、1 行を概ね 250 バイト以下に保つ（[ADR 0119](decisions/0119-agent-token-efficiency.md)）。
- 監査の指摘を直したら、当時の失敗記録は保持したまま、対応と回帰テストへの参照を追記する。

### テスト基盤

- 振る舞いや境界を追加・変更したら、通常・異常・境界の入力を既存のテストが通せるか確認する。
- 新しいアクションには `crates/echode-history/tests/properties.rs`、音を作る処理には `crates/echode-engine/tests/block_size.rs` の回帰を追加する。
- 製品ソースファイル内の単体テストが大きくなった場合（目安 100 行以上）は、`#[cfg(test)] #[path = "<module>_tests.rs"] mod tests;` で同ディレクトリの別ファイルへ分離し、製品コードの閲覧コストを抑える。
- モジュール・依存・ツールチェーン・実行入口が変わったら、`scripts/verification.json`、`Makefile`、CI、実行手順を照合する。選択条件は `scripts/verification.json` だけに置く。
  Makefile のコメントは各規則の目的を 3 行以内で書き、機能ごとの経緯や契約の一覧を積まない。
- 別プロセス・ディスク・利用者のファイルから読む入口を足したら、`crates/echode-fuzz` の既存の対象が通すか確かめ、通さなければ対象を足す。
  fuzz の発見は最小化して `regressions/<対象>/` に置き、所属の crate にも製品の境界を通す回帰を足す（[デバッグ手順](debugging/verification-and-build.md)）。
- 必須契約（`product_contracts`）は、正しいソースで実テストが 1 件通過し、誤りを戻したソースのコピーでは期待値で失敗することを確認する。
- `@MainActor` のテストは同時に動き、主スレッドを交互に使う。長い同期処理は `await` で区切るか主アクターの外へ移し、他のテストの実時間の待ちを止めない。
  画素の全面走査は `CanvasReadback.scan` を使う（[デバッグ手順](debugging.md)の `Status did not arrive`）。

### デバッグ基盤

- 新しい失敗経路・環境依存・ツールの取得エラーが判明したら、[領域別のデバッグ手順](debugging/) の該当する節へ `###` の見出しで識別方法と調査先を足し、[索引](debugging.md#症状の索引) にも同じ名前を足す。
- ログ・成果物の出力先や形式、新しいヘルパー・バンドル・外部入力を変えたら、`make doctor` の鮮度照合と `make debug-report` の収集範囲を見直す。
- 古い結果・環境不足・製品不良を区別できること、秘密情報やユーザー文書を収集しないこと、音声スレッドへ禁止操作を持ち込まないことを維持する。

### 完了時の確認

実装、説明、検査が保証する範囲、失敗時に集められる証拠が一致することを確認する。
`main` へ直接コミットし PR がないので、次の構造の確認がレビューの代わりになる（[ADR 0276](decisions/0276-priority-order-and-delegated-decisions.md#複雑さの歯止め)）。

- 同じ事実を持つ状態を増やしていない。持つ時は、どちらが正本で、どう同期するかが決まっている。
- 同じ規則を別の論理で実装していない。不変条件は `Project::check_invariants` とその述語を使い、事前確認や差分の検証も同じ述語から作る。
- 届かない分岐、場当たりの予備の処理、使われなくなった入口を残していない。
- 変更の影響範囲（呼び出し元、不変条件、保存と復旧、音の経路、検査の選択）をたどった。
- 堅牢性のための追加には、直す前に失敗する回帰テストか、再現が難しい時の検証の根拠（不変条件・サニタイザ・Miri・故障注入・静的解析）がある。
- 同じ種類の修正を繰り返していない。繰り返す時は構造を直す。

最終報告では今回の基盤の更新内容（または更新不要の理由）を簡潔に示す。

## コードの所在

### crate と主な入口

| 領域 | パス | 役割・主な入口 |
| :--- | :--- | :--- |
| 共通整数時間 | `crates/echode-time` | `TempoIndex`・区間丸め・tick / flick / sample 換算・Workgroup 期間。モデルと再生カーソルで共有し、外部依存を持たない |
| モデル・時間・不変条件 | `crates/echode-model` | `model.rs`（`Project`）、`time.rs`（`Tick` / `Flicks` / `TempoMap`）、`invariants.rs`、`hierarchy.rs`（親の検証）、`changeset.rs`、`duplicate.rs`（部分木の複製と導出 ID） |
| アクション・編集セッション | `crates/echode-actions` | `action.rs`（`Action` / 登録簿）、`session.rs`（`Session` の Undo / Redo 履歴） |
| Git 履歴・パッケージ | `crates/echode-history` | `host.rs`（文書・復旧）、`autosave.rs`（保存）、`document_git.rs`（Git と同期） |
| 保存の検証・素材 | `crates/echode-history` | `document_parse.rs`（スキーマと読込み）、`error.rs`（原因の型）、`objects.rs`（CAS）、`peaks.rs`（波形） |
| 文書・復旧・設定の失敗 | `crates/echode-history/src` | `error_document.rs`（`DocumentError`・`JournalError`・`SettingsError`）、`host_error.rs`（`HostError`・`ElementError`・参照の拒否） |
| 再生計画・カーソル | `crates/echode-realtime` | `lib.rs`（サンプル時刻の計画・鳴っている集合・ID の索引）、`event.rs`（イベント列）、`cursor_tests.rs`（境界の訪問数） |
| オーディオエンジン | `crates/echode-engine` | `playback.rs`（再生器の入口 `playback` と `PlaybackOptions`・`PlaybackAudio`・`CachedTrack`）、`plugins.rs`（別プロセス CLAP ステージ）、`bypass.rs`（ドライ平面の通過）、`mix.rs`（フェーダー補間・f64 加算）、`declick.rs`（停止減衰）、`render.rs`（オフライン書出し）、`device.rs`（機器問い合わせ）、`device_measurement.rs`（非既定 feature の実機計測） |
| CoreAudio I/O・素材読込 | `crates/echode-audio-io` | `stream.rs`（HAL 入出力）、`opening.rs`（出力準備の別スレッド・機器なしの検査用出力）、`session.rs`（文書の音声セッション）、`error.rs`（`AudioError`。境界のエラーの変種）、`clips.rs`（WAV 読込・SRC・PCM 共有キャッシュ）、`transport.rs`（取消世代）、`workgroup.rs` / `hardware.rs`（Workgroup・機器通知）、`trace.rs`（停止出力診断）、`activity.rs`（スリープ抑止）、`panic_log.rs`（コールバックの panic の捕捉と記録） |
| 共有メモリ・IPC | `crates/echode-sandbox` | `region.rs`（共有メモリ・`arm_abort_notice`）、`session.rs`（`PluginControl`）、`workgroup.rs`、`wire.rs` |
| プラグインヘルパー | `crates/echode-plugin-host` | `scan.rs`（バンドル走査）、`catalog.rs`（キャッシュ）、`threads.rs` / `events.rs`（CLAP 処理・リセット）、`awake.rs` |
| 同梱音源 | `crates/echode-sine-pad` | Sine Pad CLAP、事前領域拡張、状態と余韻。`scripts/build-bundled-plugins.sh` で通常の走査用バンドルを作る |
| 同梱 DSP・確保の検出 | `crates/echode-dsp` | `sine_pad.rs`（ホストに依存しない `SinePad`）、`allocation.rs`（`ForbidAlloc` / `GuardAlloc`）。エンジンの `TestSynth` は単体検査用のアダプター |
| 故障注入プラグイン | `crates/echode-test-plugin` | クラッシュ・ハング・ステレオ・残響・MIDI 検証用 CLAP プラグイン |
| 描画クエリ・可視列 | `crates/echode-draw` | `query.rs`（アレンジ・ピアノロールの可視列抽出・空間インデックス） |
| JSON-RPC・MCP | `crates/echode-api` | `rpc.rs` / `serve.rs`（Unix ソケット RPC）、`mcp.rs`（MCP stdio ブリッジ） |
| CLI | `crates/echode-cli` | `render`、`serve`、`mcp` サブコマンド。統合テストは `tests/render.rs`、`tests/mcp.rs` |
| 文書の手番 | `crates/echode-document` | `service.rs`（投入・取消・終了）、`worker.rs`（手番と保護）、`ticket.rs`（完了の 1 回の保証）、`error.rs`（`DocumentError`。手番・レーンの失敗の変種）、`saves.rs`（保存の取込み）、`lane.rs`（デバイスレーン）、`device.rs`、`activity.rs`、`live.rs` |
| 外部入力の fuzz | `crates/echode-fuzz` | 制御ソケット・共有領域・ジャーナル・パッケージ・音声の対象。`Target::run` を libFuzzer の `fuzz_targets/` と `tests/replay.rs` が共有し、`regressions/` の発見を通常テストで再生する（[ADR 0290](decisions/0290-fuzz-targets-for-external-input.md)） |
| 製品 C ABI | `crates/echode-ffi` | `lib.rs`、`document.rs`（C のコールバックと `Ticket`）、`snapshot.rs`（`Snapshot`・`SnapshotPresenter`）、`controls.rs`。ヘッダ生成は `cbindgen` |
| Swift ブリッジ | `swift/EchodeBridge` | `Document.swift`、`Audio.swift`、`Controls.swift`（`echode-ffi` の Swift 6 ラッパー） |
| 製品 macOS アプリ | `swift/EchodeApp` | `Sources/EchodeAppCore/`：`ProjectController.swift`、`DocumentSession.swift`、`Arrange*.swift`、`Piano*.swift`、`Mix*.swift`、`DeviceChain.swift`、`Workspace*.swift`、`Canvas*.swift` |
| スパイク（検証用） | `spikes/` | `draw`（S1）、`bridge`（UI-2）、`input`（S5）。スパイクの型を製品へ流用しない |
| 検査・診断・配布スクリプト | `scripts/` | `verify.py`、`verification.json`、`check_product_contracts.py`、`check_real_hal.py`、`diagnose.py`、`distribution.py`、`tests/` |

間引き・パック・バンドルは `echode-history/src/thin.rs`、意味上の拒否と UTF-8 の原因は `error_thin.rs` に置く。
`tests/thin_errors.rs` で公開入口から原因・文面・原本の保持を、`tests/thin.rs` で先端の保持と間引き後の読込みを検査する。

### Phase 0 の検証コード

Phase 0 の履歴モデル（`echode-history/src/project.rs`・`format.rs`・`store.rs`）は、非既定の `spike` feature 専用である。
直接その通常回帰を動かす時は `cargo test -q -p echode-history --features spike --lib --test roundtrip`、
計測は従来どおり `make spike-history` を使う。製品の型・保存形式と混同しない
（[ADR 0211](decisions/0211-opt-in-history-spike.md)）。

失敗の結果は `PackageError` を共用し、スパイク固有の `SpikeError` だけを feature に閉じる。
`tests/roundtrip/errors.rs` は公開入口から原因・文面・原本の保持と、初期化失敗後の終了を子プロセスで検査する。
`store_tests.rs` は通信路の切断と期限の種類を、待ち時間を使わず照合する（[ADR 0301](decisions/0301-typed-history-spike-errors.md)）。

エンジンの Phase 0 の `graph.rs`・`wait.rs`・`device_measurement.rs` も非既定の `spike` feature 専用である。
通常回帰は `cargo test -q -p echode-engine --features spike --lib`、実機計測は `make spike-engine-smoke`・`make spike-engine` を使う。
製品の `playback`・機器一覧・`IoWorkgroup` は feature なしで使える
（[ADR 0213](decisions/0213-opt-in-engine-spike.md)）。
`graph.rs` は計測と比べられる形を保つため、音声経路の lint を解除している（[ADR 0284](decisions/0284-audio-path-panic-lints.md)）。
製品の命令変換とリングの境界回帰は `playback.rs::QueuedCommand` と `playback_command_tests.rs` に置く。
未使用だった別系統の `handoff.rs` と専用の公開型は削除した（[ADR 0253](decisions/0253-product-only-playback-commands.md)）。

### モデルのエラー

`echode-model/src/error.rs` は `InvariantError` と `ArrangeError` の原因・表示、`model.rs` は編集・組立ての `ModelError` を持つ。
規則は引き続き `invariants.rs` と既存の述語が正本で、表示のために検査を重ねない。
`invariants_error_tests.rs` は既存の不変条件テストの fixture を共有し、組立て・並びの型と `Error::source` を照合する。
パッケージと復旧を通る原因は履歴の `tests/package_errors.rs`・`tests/audit_recovery.rs` で照合する（[ADR 0303](decisions/0303-typed-model-invariant-errors.md)）。

### アクションの引数

`echode-actions/src/parameters.rs` は `ParameterError` と、項目の型から wire の型・モデルの検証へ写す `Parameter` を持つ。
`action.rs::define_actions!` が要求の形と変換を登録簿から生成する。呼出側は `Command::from_value` を使い、Serde の表示文で原因を分類しない。
`tests/parameters.rs` は任意 ID・flatten・整数端点・検証の順、API の `rpc_tests.rs` は公開応答、履歴の `tests/document_errors.rs` は型と取消・Redo・ジャーナルの保持を検査する
（[ADR 0306](decisions/0306-typed-action-parameters.md)）。

### モデルの項目表と変更セット

パラメーターの変更セットの生成器は `echode-model/src/changeset_parameter.rs`、
ビット単位の差分・作成・削除・逆適用とジャーナルの回帰は履歴の `tests/parameter_changesets.rs` に置く。
任意のビットを扱う低水準 API と、公開前の不変条件・保存時のゼロの正規化を区別する。
同じミキサー内のパラメーター ID の一部・全部の置換も、同じ回帰で作成・順序更新・逆順の削除と正逆適用を検査する。
2 ミキサーの全置換パターンと同時の値変更、置換レコードの任意ビットを、独立したフィールド列とジャーナルで照合する。
操作列全体の往復は `tests/properties.rs` で検査する（[ADR 0214](decisions/0214-parameter-changeset-fields.md)）。
パラメーターの項目表は `echode-model/src/parameter_fields.rs` でモデル宣言・等価比較・Serde とも共有する。
公開パッケージのキー順・拒否時の保持・旧版の移行とコミット再読込みは履歴の `tests/parameter_package.rs` に置く。
形の復号とプロジェクトの受入を区別し、保存の formatter は履歴に残す（[ADR 0222](decisions/0222-parameter-package-fields.md)）。
所属と子の列の完全性は `tests/parameter_ownership.rs` で、不明な所有者・未列挙・重複・ID 衝突を検査する。
復旧と `DocumentHost::open` の拒否、edit / undo / redo、原本と先端・ロック解放・明示修復後の再オープンも同じ回帰に置く。

デバイスのスカラー項目の表は `echode-model/src/device_fields.rs` でモデル宣言・Serde とも共有する。
変更セットの生成器は `echode-model/src/changeset_device.rs`、独立したフィールド順・省略値・異常入力の回帰は
履歴の `tests/device_changesets.rs` に置く。操作列は設定済み CLAP デバイスからも始める。
モデル値だけを検査し、プラグインを読み込まない（[ADR 0215](decisions/0215-device-changeset-fields.md)）。
公開保存のキー順・改行、拒否時の原本保持、状態オブジェクトの検証・旧版の移行とコミット再読込みは
履歴の `tests/device_package.rs` に置く。保存の配置だけを扱う formatter は `src/document_device.rs` に残す
（[ADR 0223](decisions/0223-device-package-fields.md)）。

所属数の復元検証は `echode-model/src/model_device_apply_tests.rs`（`model_scale_tests::device_apply`）でデバイスの訪問数を数える。
`Project::apply_changeset` の作成・削除・値変更・チェインの並替えと正逆適用で、適用後のデバイス数以内に制限する。
子の未列挙・欠けた ID・誤った所有者・所有者だけの削除の拒否と原本保持も検査し、プラグインや機器は開かない。

ノートのスカラー項目の表は `echode-model/src/note_fields.rs`、独立した整数・参照・フィールド順の回帰は
履歴の `tests/note_changesets.rs` に置く。所属と中身の順序、必須項目・型・未知の ID の拒否とジャーナルも検査する。
操作列は共有内容と 16 bit の発音・リリースの強弱を持つ初期状態からも始める。
低水準の任意値の往復は公開前の不変条件検査と区別する（[ADR 0216](decisions/0216-note-changeset-fields.md)）。
モデル宣言・Serde と変更セットもこの表を共有し、保存の型と手書きの変換は持たない。
公開保存のキー順・1 ノート 1 行・整数の端点・旧版の段階変換と拒否時の原本保持、
共有内容とコミットからの再読込みは履歴の `tests/note_package.rs` に置く
（[ADR 0224](decisions/0224-note-package-fields.md)）。

トラックのスカラー項目の表は `echode-model/src/track_fields.rs`、変更セットの生成器は `changeset_track.rs` に置く。
独立した名前・親・兄弟キーとフィールド順の回帰は履歴の `tests/track_changesets.rs` を使う。
子の ID 列と解決、親の木・分数キーの拒否とジャーナルも検査し、操作列は入れ子のフォルダとノート・デバイスを持つ子からも始める
（[ADR 0217](decisions/0217-track-changeset-fields.md)）。
モデル宣言と保存用の `TrackMetadata`・変換もこの表から生成する。
公開保存のキー順・改行・旧版・拒否時の原本保持と部分木・コミット再読込みは `tests/track_package.rs` に置く。
保存ヘッダの配置だけを扱う formatter は `src/document_track.rs` に残し、ID・子・階層の検証は省略しない
（[ADR 0225](decisions/0225-track-package-fields.md)）。

トラック追加の一括 ID 照合は `model.rs::track_ids_free`、訪問数と衝突の回帰は
`model_track_insert_tests.rs`（`model_scale_tests::track_insert`）に置く。
末尾・子・指定位置への追加で、既存ノートの ID を一度だけ訪ね、候補内と全種の既存 ID との衝突拒否を保つ。

クリップ作成の候補 ID 照合も同じ走査を使う。音声は `model_audio_insert_tests.rs`（`model_scale_tests::audio_insert`）、
ノートは `model_insert_tests.rs`（`model_scale_tests::insert`）で、通常・衝突・エラー順・変更セットの正逆適用と走査回数を検査する。
`spike-scale` の `model_edit.place_audio_p50_ms`・`new_midi_clip_p50_ms` は、同じ版への各 20 回の追加を測り、素材・機器は開かない。

ノート複製・ベロシティ変更の内容探索は `model_note_lookup_tests.rs`（`model_scale_tests::note_lookup`）で比較回数を数える。
先頭・末尾の共有内容、異なる配置原点、欠けた内容・対象、未変更の共有と変更セットの正逆適用を検査する。

ベロシティの同値設定・欠けた対象での複製回避は `model_note_velocity_tests.rs`（`model_scale_tests::note_velocity`）で内容とノートの複写数を数える。
16 bit 値の差・共有配置・元の版の保持・変更セットの正逆適用、履歴の `tests/note_velocity.rs` で同値設定後の Undo / Redo とジャーナルを検査する。

ノート複製の実際の内容複写は `model_note_duplicate_tests.rs`（`model_scale_tests::note_duplicate`）で数える。
単体テスト専用の `Content::clone` は項目表から同じフィールドを複写し、製品ビルドは従来の導出実装を保つ。
結合しない固定・自動配置と別トラックの固定リンクで、内容は一度だけ複写し、窓の拡張・原点・正逆適用を保つ。

幾何編集の内容複写は `model_note_edit_tests.rs`（`model_scale_tests::note_edit`）で、同値と欠けた対象を 0 回、結合しない値変更を 1 回に制限する。
別原点の固定リンク・強弱・窓と正逆適用、同じ幾何でも自動配置の再適合と結合が残ることを検査する。
`model_scale_tests::note_duplicate` は同じ幾何で追加したノートの整列も検査し、コピー回避で新規ノートの順序を壊さないことを確認する。

ノート列そのものの複写は `model_note_refit_tests.rs`（`model_scale_tests::note_refit`）で、単体テスト専用の `Note::clone` から数える。
項目表を共有して導出実装と同じフィールドを保ち、内容の copy-on-write 以外の一時コピーも計数する。
結合しない同値の自動配置編集は 0、変更時は内容のノート数（複製では対象の 1 ノートも加算）、結合は元の各ノート 1 回に制限する。
負・小節未満の原点、別トラックの固定リンク、窓の拡縮・16 bit の強弱・未変更共有と正逆適用も検査する。

中身の音声参照・テイク ID 列の表は `echode-model/src/content_fields.rs`、独立したメタデータ・ID 列と順序の回帰は
履歴の `tests/content_changesets.rs` に置く。ノートの単一差分・全 ID 列、旧 null 音声・`Lane` の無変更互換、子の解決も検査する。
操作列は共有ノート・音声参照・空でないテイク列からも始める。音声・プラグイン・機器は開かない
（[ADR 0218](decisions/0218-content-changeset-fields.md)）。
内容同一性の探索量と未変更ノートの読み飛ばしは `echode-model/src/model_diff_tests.rs`（`model_scale_tests::diff`）で数える。
内容のキー訪問を前後の内容数の和以内に制限し、内容 ID の全交換・部分交換、既存内容間のノート移動と変更順・正逆往復も検査する。
`spike-scale` の `model_edit.diff_ms` は変更セット生成だけを測り、モデル編集・適用・保存と区別する。

配置の項目表は `echode-model/src/placement_fields.rs`、独立した整数・ゲインビット・フィールド順の回帰は
履歴の `tests/placement_changesets.rs` に置く。所属とトラックの配置 ID 列、中身の存在・異常入力・レーンの互換も検査する。
操作列は固定・共有配置、負のオフセット・小さいゲイン・非既定フェードからも始める。素材・機器は開かない
（[ADR 0219](decisions/0219-placement-changeset-fields.md)）。

テンポ点のテンポ項目の表は `echode-model/src/tempo_fields.rs`、独立した分数・tick 順とジャーナルの回帰は
履歴の `tests/tempo_changesets.rs` に置く。必須項目・型・未知の tick・二重作成・空マップと旧作成規則も検査する。
操作列は負の位置・グリッド外の位置と分数テンポからも始める。実時間変換・素材・機器はこの回帰の対象外とする
（[ADR 0220](decisions/0220-tempo-changeset-fields.md)）。
テンポ点の訪問数は `echode-model/src/model_tempo_diff_tests.rs`（`model_scale_tests::tempo_diff`）で数える。
共有配列は 0 回、別配列は前後の点数の和以内とし、独立した時刻マップとの変更順の照合と正逆往復を検査する。

プロジェクトの拍子の表は `echode-model/src/project_fields.rs`、独立した分子・分母と更新順の回帰は
履歴の `tests/project_changesets.rs` に置く。型・キーの拒否と旧トラック ID 配列の無変更互換も検査する。
操作列は非 `4/4` の空の階層と固定配置のある状態からも始める。低水準の適用は拍子アクションを再実行しない
（[ADR 0221](decisions/0221-project-changeset-fields.md)）。

項目を追加する時は、その `*_fields.rs` と意味を持つ編集規則・不変条件・移行を変更する。
新しいプロパティ名は `changeset.rs` の登録簿へ、新しい値の種類だけは `change_values.rs` へ加える。
保存・復元の転記や FFI の安全なスカラー問い合わせを別に手書きしない。
デバイス・トラック・配置の表の投影指定は、公開しない項目も明記する。C ABI の追加は別に設計・検証する。
残りの保存のレイアウトと原本保持は履歴の `tests/schema_fields.rs`、全値の wire 表記は `tests/change_value_wire.rs`、
保存再読込みから保持した ABI の値は FFI の `snapshot_fields_tests.rs` に置く
（[ADR 0226](decisions/0226-model-field-registry-completion.md)）。

### JSON-RPC・MCP

API・MCP の通信のエラーは `crates/echode-api/src/error.rs` の `ApiError` と `ApiIoOperation`、
RPC の文書・引数の原因は `rpc.rs::PerformError`、MCP のリソースの原因は `mcp.rs::ProtocolMessage` に置く。
元の原因を保持し、応答の生成時だけ文にする（[ADR 0304](decisions/0304-typed-api-errors.md)）。
`tests/typed_errors.rs` は公開入口の I/O・パッケージ・パス保持、`rpc_tests.rs` と `mcp_tests.rs` は内部の原因と wire 応答を照合する。

JSON-RPC の外枠検証は `crates/echode-api/src/rpc.rs::ApiSession::reply` に置く。
公開 API・実ソケットの `tests/request_validation.rs` は、不正な ID・版・params の拒否と、
通知の応答省略・後続要求・ジャーナルと Undo / Redo・再オープンを検査する
（[ADR 0020 の後続修正](decisions/0020-json-rpc.md#要求の外枠の検証2026-10-29)）。

JSON の構文上の空白は `ApiSession::reply` の `trim_matches` と既存の JSON パーサーで検証する。
`tests/request_whitespace.rs` は公開 API・実ソケットで不正な前後の空白を拒否し、ジャーナル・Undo / Redo・再オープンを保つ。
正しい空白・文字列内の Unicode と空白だけの行も検査する
（[ADR 0020 の空白検証](decisions/0020-json-rpc.md#json-の空白だけを除去する2026-10-29)）。

ソケットの改行完了は `serve.rs::serve_connection_limited` が検証する。
`tests/request_framing.rs` は EOF の部分行で編集・通知・Undo / Redo を実行しないことと、
完了済みの前方の編集・ジャーナル・既存の履歴・再オープン、CRLF と分割受信を検査する
（[ADR 0073 の改行境界](decisions/0073-socket-path.md#改行前の切断の拒否2026-10-29)）。

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

新 MCP の要求ごとの版・capabilities は `mcp.rs::validate_modern`、公開入口と実履歴の回帰は `tests/mcp_metadata.rs` に置く。
旧 MCP の初期化前・完了後とも不正な編集を送らず、ジャーナル・既存 Redo と正当な一括編集の単一 Undo / Redo・再オープンを保つ。
メタデータの省略時に直前の新 MCP 要求から版や準備完了を引き継がないことも検査する
（[ADR 0115 のメタデータ回帰](decisions/0115-phase2-mcp.md#新-mcp-の要求ごとのメタデータの回帰補強2026-10-29)）。

Undo の段が持つ分の推定は `echode-model/src/footprint.rs`、段の並びをたどる集計は `echode-actions` の `Session::usage` に置く。
中身のノートの共有の run（`Notes`）は `echode-model/src/notes.rs` にあり、区切りの規則と共有の回帰は `notes_tests.rs` にある（[ADR 0287](decisions/0287-shared-note-runs.md)）。
ノートを変える処理は `splice`・`update`・`insert_sorted`・`replace_sorted` を使い、全体を写す `edit` は多くのノートを変える時だけにする。
`history.usage` は `DocumentHost::history_usage` を文書のスレッドで答え、場面ごとの計測は `crates/echode-actions/tests/history_usage.rs` にある
（[ADR 0286](decisions/0286-history-usage-diagnostics.md)）。

MCP のツール名によるアクション種別の決定と引数のローカル拒否は `mcp.rs::call_tool` に置く。
`tests/mcp_arguments.rs` は旧・新 MCP で種別上書き・余分な引数・不正な型と未知のツールを接続前に拒否し、
ジャーナル・既存 Redo・後続の正当な編集の単一 Undo / Redo と再オープンを検査する
（[ADR 0115 の引数回帰](decisions/0115-phase2-mcp.md#ツール引数のローカル拒否の回帰補強2026-10-29)）。

### プラグインのヘルパーと走査

読込み・生成・起動の `LoadError` と clack の型への対応は `echode-plugin-host/src/launch.rs`、共有する原因と通信は `echode-sandbox/src/launch_failure.rs` に置く。
`reject_launch` は `LoadError` だけを受け、実行全体の `HostError::Load` は元の原因を保つ（[ADR 0310](decisions/0310-typed-plugin-load-boundary.md)）。
回帰はそれぞれの `*_tests.rs` と、`tests/isolation.rs` の `launch_` で実ヘルパーを通す。
不正な起動拒否の `LaunchDecodeError` と `LaunchField` も `launch_failure.rs` に置く。
実ソケットの回帰は `control_launch_tests.rs`、公開操作での原因の保持は `session_operations_tests.rs` を使う（[ADR 0313](decisions/0313-typed-launch-refusal-decoding.md)）。
拒否の構造の `RefusalDecodeError` と `RefusalField` は `refusal_decode.rs`、共通の固定長の検証と読込みは `refusal.rs` に置く。
実ソケットの全拒否経路は `control_refusal_tests.rs`、公開操作での原因の保持は `session_refusal_tests.rs` で照合する（[ADR 0314](decisions/0314-typed-refusal-structure-errors.md)）。
拒否以外の制御メタデータの `ControlDecodeError`・`ControlField` と固定長の読込みは `control_decode.rs` に置く。
実ソケットの長さ・種類・ポートの列の回帰は `control_decode_tests.rs`、公開操作での原因の保持は `session_decode_tests.rs` を使う（[ADR 0315](decisions/0315-typed-control-metadata-errors.md)）。
パラメーター・通知・窓の報告の内側の `PayloadDecodeError`・`ParamPayload`・`ParamField` と、列を読む `Cursor` は `payload_decode.rs` に置く。
新しい列は `Cursor::list` で読み、手書きのループと確保を足さない。フィールドを足したら `control_payload_tests.rs` の並びのモデルにも足す。
実ソケットの回帰は `control_payload_tests.rs`、公開操作での原因の保持は `session_payload_tests.rs` を使う（[ADR 0316](decisions/0316-typed-parameter-payload-errors.md)）。
記述子の規則の違反の `DescriptorRuleError` と、1 枠を超えるメッセージを `PART` の枠に分ける送信は `control_transport.rs` に置く。
違反の判定は `control.rs` の `descriptor_rule`、部分をつなぐのは `Socket::parse_one` である（[ADR 0318](decisions/0318-split-control-messages.md)）。
回帰は `control_rights_tests.rs`・`control_send_tests.rs` と `session_operations_tests.rs`、製品のヘルパーでは `isolation.rs` の `a_state_longer_than_one_frame_saves_and_restores_in_parts` を使う。
この crate の I/O エラーに文字列リテラルだけを渡すと、`check_typed_errors.py` が拒否する（[ADR 0317](decisions/0317-typed-control-transport-errors.md)）。
予約済み ID と内部の応答不一致の共有型・通信は `refusal.rs` と `main_reply.rs` に置く。
実際の応答の種類への変換は `main_requests.rs::unexpected` にまとめる。
回帰は `refusal_tests.rs`・`main_requests_tests.rs` と、実ヘルパーを通す `tests/isolation.rs` の `invalid_parameter_id_` を使う（[ADR 0309](decisions/0309-typed-helper-request-failures.md)）。

共有メモリの作成と `SharedMemoryOperation` は `echode-sandbox/src/region.rs`、
操作・errno・表示・unlink と close-on-exec の回帰は `region_error_tests.rs` に置く（[ADR 0311](decisions/0311-typed-shared-memory-errors.md)）。

プラグインのメインスレッド要求の所有は `echode-plugin-host/src/main_requests.rs`、実行は `threads.rs::perform_main_request` に置く。
要求番号付きの制御フレームは `echode-sandbox/src/control.rs`、親の応答照合と背景保存は `session.rs` に置く。
回帰は `main_requests_tests.rs`・`session_correlation_tests.rs`・`control_tagged_tests.rs` と `tests/audit.rs` の SPG-01 を使う。
制御文字列の共通の UTF-8 検証と `ControlTextError` は `control_text.rs`、実ソケットの全経路の回帰は `control_text_tests.rs` に置く。
公開するパラメーター操作で原因を保持する回帰は `session_operations_tests.rs`、不正・未完の列の fuzz の種は `echode-fuzz/src/control.rs` にある（[ADR 0312](decisions/0312-typed-control-text-errors.md)）。
受けた記述子の扱い（全部を持ち、2 個以上なら閉じて拒否する）の回帰は `control_rights_tests.rs` に置く。
パイプの書込み側の写しを送り、読み側の終端で残りを判定する（[ADR 0288](decisions/0288-received-descriptors-are-all-owned.md)）。
エンジンのテスト専用の素通しではなく、製品と同じ `device_control_handle` を通す
（[ADR 0199](decisions/0199-owned-helper-control-requests.md)）。

制御専用ハンドルの型付き操作は `echode-sandbox/src/session_operations.rs` に直接実装し、
同名の `ControlState` や `PluginHost` の素通しを増やさない。
公開操作と通信の対応・拒否・背景保存の順序は `session_operations_tests.rs`、
要求番号・古い応答と保存の世代は `session_correlation_tests.rs` を使う
（[ADR 0227](decisions/0227-control-only-plugin-operations.md)）。

コールバックの費用がトラック数とともにどう増えるかの release 計測は `crates/echode-plugin-host/tests/track_scale.rs` に置き、`make spike-plugin-tracks` で動かす。
ブロックのイベントの振り分けは `playback.rs` の `group_events`、ヘルパーのトラックのキーの索引は `plugins.rs` の `HostTracks` である（[ADR 0236](decisions/0236-callback-work-per-track.md)）。
楽譜の大きさに比例する処理（ループの折り返し、再生中の編集、制御側の作り直し、モデルの探索）の release 計測は `crates/echode-engine/tests/scale.rs` に置き、`make spike-scale` で動かす（[実行指示](reviews/2026-11-03-fix-instructions.md)）。
イベントの局所的な容量上界は `playback_capacity.rs`、通常の境界回帰は `playback_capacity_tests.rs` と `tests/audit_scale_and_mix.rs` に置く。
共有領域の密度・拡張・旧発音の解放は `hosted_playback/event_capacity.rs` で製品バッファへ通す（[ADR 0240](decisions/0240-block-local-event-capacity.md)）。

取りこぼしの継ぎ目は `plugin_declick.rs`、実ヘルパーの保留と復帰は `hosted_playback/deadline_fade.rs`、分割長比較は `tests/block_size/plugin_deadline.rs` に置く
（[ADR 0245](decisions/0245-plugin-deadline-fades.md)）。

ヘルパーの記述子の上限を上げる処理は `echode-sandbox/src/descriptors.rs` に置き、回帰は上限を変えるため専用のバイナリ `tests/descriptor_limit.rs` にする（[ADR 0237](decisions/0237-helper-descriptor-limit.md)）。

プラグイン走査の期限・パイプ排出は `crates/echode-plugin-host/src/scan.rs`、回帰は `tests/scan.rs` に置く。
JSON・子孫のパイプ保持・ハング等のネイティブ検査専用バイナリは `tests/support/scan_helper.rs` とし、製品へ同梱しない
（[ADR 0190](decisions/0190-native-scan-deadline-fixtures.md)）。fixture はパスのファイル名で動作を選ぶ。
何を壊れたと記録するかは `ScanError::is_verdict` の 1 か所で決め、常駐のカタログの 2 回の走査は `live.rs` の `scan_due`、その順番の回帰は `live_tests.rs` に置く
（[ADR 0285](decisions/0285-scan-timeouts-are-not-verdicts.md)）。

### FFI・文書・復旧・編集型

目録の Rust の境界は `crates/echode-ffi/src/catalog.rs` の `CatalogError` で、未検出のヘルパーのパスと元の目録・JSON の原因を持つ。
回帰は `catalog_tests.rs` が子プロセスの環境・走査先を隔離し、同じ入口と C ABI の数値・JSON を照合する（[ADR 0302](decisions/0302-typed-ffi-catalog-errors.md)）。

文書の単一編集は `worker.rs::apply_edit` から `Command` の `FromStr` を使う。
`error.rs` は意味の拒否を `DocumentError::Parameters`、JSON の構文・形を元の原因付きの `Malformed` へ分ける。
`crates/echode-document/tests/edit_errors.rs` は製品ワーカーで原因・入力・診断・拒否後の履歴と再オープンを検査する（[ADR 0307](decisions/0307-typed-document-command-decoding.md)）。

フォルダ表示の製品 ABI は `crates/echode-ffi/src/projection.rs`、Swift の保持と階層取得は
`swift/EchodeBridge/Sources/EchodeBridge/TrackHierarchy.swift` に置く。
ヘッダの開閉と表示行の共有は `ArrangePaneView.swift`、追加位置の保持は `TrackInsertion.swift` を使う。
複製対象の保持は `TrackDuplication.swift`、ヘッダの右メニューは `ArrangeHeaderView.swift` に置く。
CLAP パラメーターの通信は `crates/echode-sandbox/src/params.rs`、ヘルパー側は `threads.rs` の `flush_params` / `param_infos`、
文書の手番は `crates/echode-document/src/device.rs`（検査・往復・記録）と `lane.rs`（ワーカー外のデバイスレーン）に置く。
画面は `DeviceParameters.swift`（デバイスごとの状態・世代・ピン留め）、`WorkspaceParameterView.swift`（画面内の行だけの一覧）、
`WorkspaceDeviceView.swift`（横並びの列とドロップ）に置く。
列の開閉と文書 ID の検査は `DeviceDisclosure.swift`、幅・入力・フォーカスの制御は `WorkspaceDeviceView.swift` に置く。
ヘッダの FX 名前と残りのチェインのメニューは `TrackFXNames.swift`、入力とアクセシビリティは `ArrangeHeaderView.swift` に置く。
テスト用 CLAP のパラメーターは `crates/echode-test-plugin/src/params.rs` が持つ。
プラグイン独自のウィンドウは、ヘルパーの `crates/echode-plugin-host/src/gui.rs`（窓と規則）・`gui_appkit.rs`（AppKit とキー入力）と `relay.rs`（出力イベントのリング）、
通信の `crates/echode-sandbox/src/gui.rs`、`crates/echode-document/src/activity.rs`（文書単位の受け口）に置く。
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
準備を待つデバイス要求の順序は `crates/echode-document/src/worker.rs` の `waiting` と `device.rs` の `Waiting` に置く（[ADR 0136](decisions/0136-output-opening-off-worker.md)）。
開いた出力の置換グラフの準備スレッドと回収は `crates/echode-engine/src/playback_preparation.rs`、
要求の集約と公開は `playback.rs` の `publish_pending` に置く（[ADR 0142](decisions/0142-background-plugin-graph-preparation.md)）。
停止中の置換の素材読込みスレッドと `MediaInputs` は `crates/echode-engine/src/playback_media.rs`、
再生の保留と `audio.prepare` は `crates/echode-audio-io/src/session.rs`、開いた時の送信は `ProjectController.preparesOutput` に置く
（[ADR 0147](decisions/0147-output-prewarm-and-background-media.md)）。

古い保存版への復旧通知と書込み禁止は `DocumentHost::recovery_notice`・`accept_recovery`、
受渡しは `crates/echode-document/src/worker.rs` の `history_report` の `recovery` と `history.accept_recovery` に置く。
Swift の警告と開く要求の取消は `RecoveryConsent.swift`、公開前の確認は `ProjectController.open`、
画面の選択肢は `WorkspaceView.swift`、回帰は `recovery_tests.rs`・`RecoveryConsentTests.swift` に置く
（[ADR 0183](decisions/0183-recovery-consent.md)）。

時間単位とモデル範囲の受渡しは `crates/echode-ffi/src/constants.rs` の `echode_model_constants`、
Swift の値と拍の変換は `EchodeBridge/ModelLimits.swift` の `ModelLimits`・`Tick` に置く。
回帰は ABI の `tests/model_constants.rs`、橋渡しの `ModelLimitsTests.swift`、製品の `ModelUnitsTests.swift` に置く
（[ADR 0200](decisions/0200-shared-model-units-and-limits.md)）。

編集型の生成器は `crates/echode-actions/examples/swift_actions.rs`、生成物は橋渡しの `EditAction.swift` に置く。
`make swift-actions` で再生成し、`make check-swift-actions` は変更せず照合する。後者は `make check` でも行う。
全操作の共有 fixture は `EchodeBridgeTests/Fixtures/edit-actions.json`、符号化は `EditActionTests.swift`、
製品ワーカー・履歴・再オープンは `TypedEditTests.swift`、ピアノロールの型付き controller は `TypedPianoEditTests.swift` で検査する。
controller の単一／一括編集はすべて生成型へ移行した。辞書で要求を組まない。
トラック・デバイスの対象・選択・アンカーと末尾・CLAP ID 境界・履歴と再オープンは `TypedTrackDeviceEditTests.swift` で検査する
（[ADR 0202](decisions/0202-generated-swift-edit-actions.md)）。
ミキサーと音声クリップのゲイン・フェードは `TypedMixerEditTests.swift` で、試聴の先行確定・精度・履歴・拒否と保存を検査する。
`performActions` の `finishingPreview` は共通の `applyEdit` へ渡し、試聴終了時だけ通常編集の排他を越える。
クリップ移動・複製と音声配置は `TypedClipEditTests.swift` で、整数 tick・入力先・選択・結果参照・履歴・拒否と再オープンを検査する。
一括編集内で前の結果を使う要求は `ReferencingEditAction` で組み、`EditText.result` と `performActions(referencing:)` で送る。

### GUI の書出し

- CLI の失敗は `echode-cli/src/error.rs` の `CliError`・`ArgumentError`・`CliIoOperation`。公開入口は `tests/typed_errors.rs`、原因の転送と分類は `src/error_tests.rs` で照合する。
  プラグインのブロック拒否はエンジンの `error.rs::PluginBlockFailure` に ID と種類を持つ（[ADR 0305](decisions/0305-typed-cli-errors.md)）。
- 設定とパッケージ内の記憶・旧パス設定の読取りは `ExportSettings.swift`、専用パッケージの準備と別プロセスの寿命は `ExportJob.swift`、画面は `WorkspaceExportView.swift`。
- 数値入力の下書きと全入力の検証は `ExportSettingsDraft.swift`。`ExportInputTests` が実シートのフィールドエディターを通し、`ExportSettingsDraftTests` がロケール・有限性・丸め境界を確認する。
  必須契約は `export-input-ui`（[ADR 0262](decisions/0262-export-numeric-drafts.md)）。既存の `check-swift`・`check-product-contracts` から実行する。
- 共通の既定設定も `ExportSettings.swift` が版付きで保持する。`WorkspaceExportView` の保存・適用・初期化は共通値と下書きだけを変え、前回値・履歴を変えない。
  `ExportDefaultsTests` と必須契約 `export-defaults-ui` が優先順・不正値・文書交換を検査する（[ADR 0263](decisions/0263-common-export-defaults.md)）。
- 名前付き書出しプリセットの保存と検証は `ExportPresets.swift`、シートの操作と表示状態は `ExportPresetControls.swift`。
  `ExportPresetTests`・`ExportPresetStorageTests` と必須契約 `export-presets-ui` が入力・重複拒否・削除・不正保存物の保持・文書交換を検査する（[ADR 0264](decisions/0264-named-export-presets.md)）。
- `HistoryCommand.exportSnapshot` が文書ワーカーへ渡り、`echode-history/src/export.rs` が記録済みの `Project` と参照するオブジェクトだけをコピーする。
- `ExportTests.swift`・`ExportBoundaryTests.swift` と履歴の `tests/export.rs` が、元文書との独立・設定・拒否・失敗時の保持を検査する。
  必須契約は `export-snapshot-ui`、判断と残る範囲は [ADR 0254](decisions/0254-gui-snapshot-export.md)。
- 進捗の通知は CLI の `progress.rs`、改行の復号と後退拒否は `ExportProgress.swift`。
  `ExportProgressTests` の `export-progress-ui` が終了前の UI 更新・診断との分離・失敗時の 100% 拒否を検査する。
- 途中取消は CLI の `cancellation.rs` と `ExportJob.cancel()`。`ExportCancellationTests` と CLI の `tests/export_cancellation.rs` が原本保持・文書交換・閉じた進捗を通す。
  公開前の保全と GUI の制御の引渡しは必須契約 `export-cancellation-publish`・`export-cancellation-ui`（[ADR 0256](decisions/0256-export-cancellation.md)）。
- 任意のサンプルピークノーマライズは CLI の `normalization.rs` が 1 回のミックスを一時保存し、共通ゲインを量子化前に適用する。
  `export_normalization.rs`・`normalization_tests.rs` と `ExportNormalizationTests` が、左右比・分割・設定互換・実 CLI への引渡しを検査する。
- 前回設定の保存は `echode-history/src/export_settings.rs`、文書の入口は `history.export_settings`。
  `ExportSettingsTests` と履歴の `tests/export_settings.rs`、必須契約 `export-settings-ui` が移動・移行・履歴からの独立を検査する（[ADR 0257](decisions/0257-portable-export-settings.md)）。
- CLI の出力先の境界は `echode-cli/src/destination.rs`。`tests/export_destination.rs` と必須契約 `export-destination-cli` が復旧前の拒否と全ファイル保持を確認する。
  既存の CLI 回帰の独立した出力先と後片付けは `tests/support/output.rs` が所有する（[ADR 0259](decisions/0259-export-destination-boundary.md)）。
- GUI の出力先は `ExportJob.start` が準備前に固定する。`ExportDestinationTests` と必須契約 `export-destination-ui` がフォルダのリンク切替・内外の葉リンク・原本保持を検査する
  （[ADR 0260](decisions/0260-pinned-gui-export-destination.md)）。
- 完了後の Finder 表示は `ExportJob.completedOutput` と `revealOutput`、操作子は `ExportStatusView`。
  `ExportRevealTests` と必須契約 `export-reveal-ui` が固定パス・文書交換・再試行の境界を機器なしで確認する（[ADR 0261](decisions/0261-reveal-export-output.md)）。

### ループ・値の操作子・ミックス・ヘッダ

ループの下書きは `ProjectController.loopRange` の整数範囲を両ルーラーと送信で共有する。
拍の文字列は `LoopEntry.swift` が入力境界だけで持ち、未変更の欄は元の tick をそのまま返す。
回帰は `LoopDraftTests.swift`。精度外の端点・不正入力・旧文書の拒否・再オープン時の初期化を通す
（[ADR 0201](decisions/0201-integer-loop-drafts.md)）。

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
フォルダの実効ミュート・ソロは `playback_faders.rs` が制御側で親子から解決する。
回帰は `audit_scale_and_mix.rs` と `block_size/folder_mix.rs`（[ADR 0244](decisions/0244-folder-mute-and-solo-gates.md)）。

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
ヘッダの一括ドラッグ・既定値リセットは `MixerFaderBatch.swift` が押下時の全対象・元の値を捕捉し、共通差分と確定前の全体照合を行う。
`MixerDragBatchTests.swift`・`MixerDragBoundaryTests.swift`・`MixerDragAudioTests.swift` が実入力・描画・送信集約・履歴を通す。
音声側の完全な集合と制御側での解放は `playback.rs`、出力準備中の集合保持は `session.rs` と `opening.rs` に置く。
製品 PCM のブロック長比較は `block_size.rs` の `contract_group_fader_previews_are_atomic_and_block_independent_under_rtsan` を使う
（[ADR 0228](decisions/0228-selected-track-live-mix.md)）。
横移動ゼロの音量保持は `MixScale.dragged` の恒等分岐を通す。
回帰は `MixerDragIdentityTests.swift`。縦ドラッグと解放時の原点復帰で、表示下限未満の値・Undo / Redo・保存を保持する。

クリップの直交ドラッグと解放時の原点復帰は `ClipDragIdentityTests.swift` で検査する。
非正規化数・端数 flick・長いフェード、Redo と実パッケージの再オープンを通す。
保存の浮動小数点表記は `document.rs::format_gain`、独立した保存回帰は履歴の `tests/float_roundtrip.rs` に置く
（[ADR 0196](decisions/0196-lossless-package-floats.md)）。同じ回帰で `DocumentHost` の増分保存・コミットからの再オープン・履歴復帰と Undo / Redo も通す。

### 素材・保管庫・波形キャッシュ

保存済み音声の有限性は `crates/echode-history/src/media.rs::decode_pcm`、公開読込みとトリム後の素材準備の回帰は
`crates/echode-audio-io/tests/pcm_validation.rs` に置く（[ADR 0056 の後続修正](decisions/0056-audio-import.md)）。

配置の未スケール PCM とピークの共有は `clips.rs`、ゲインとモノの昇格は `echode-engine/src/playback.rs::add_stored_clips` に置く。
回帰は `clips_tests.rs` と `echode-engine/tests/block_size/clip_gain.rs`（[ADR 0241](decisions/0241-unscaled-placement-pcm.md)）。

外部音声の取込み前の有限性は `media.rs::decode_file`、公開復号・取込みと履歴の回帰は
`crates/echode-history/tests/audio_import_validation.rs` に置く。
IEEE float WAV の異常値・有限値のビット保持・原本とパッケージの保持・一括編集の巻戻しと再オープンを通す。

保管庫とキャッシュの Git 除外は `objects.rs::ensure_store_gitignore`、回帰は履歴の `tests/store_gitignore.rs` に置く。
公開取込みとホスト開始で、コメント・似たパスの誤認拒否、CRLF・既存テキストの保持と二重実行を通す
（[ADR 0031 の後続修正](decisions/0031-content-addressed-store.md#除外規則の行単位の照合2026-10-29)）。

波形キャッシュの構造・バイト長・列の検証は `peaks.rs::read_cache`、公開読込みと再生成の回帰は
`crates/echode-history/tests/peak_cache_validation.rs` に置く（[ADR 0058](decisions/0058-waveform-peaks.md)）。
有限ステレオの平均とキャッシュの版は `stereo_waveform.rs` で検査する。
構築・マップの極大値と非正規化数、旧キャッシュの再生成と PCM の保持を製品の公開波形経路へ通す。
境界の途中をまたぐ列の無音は `waveform_silence.rs` で検査する。
素材前・EOF 後・配置外の包絡、粗い列とマップの端・テンポ変化とキャッシュ再利用を公開経路へ通す。

### クリップ・再生開始位置・ルーラー

クリップの移動と複製移動のモデルは `crates/echode-model/src/clip_move.rs`（置き換えの `land` と導出 ID）、
画面のドラッグ・影・要求（`ClipMove`）と controller の編集は `ArrangeClipMove.swift`、押下の振り分けは `ArrangeView.swift` に置く。
回帰は `clip_move_tests.rs`、履歴と CLI の `clip_move`、`ClipMoveTests.swift`（[ADR 0154](decisions/0154-clip-move-and-duplicate.md)）。
移動時の ID 集合の収集回数と共有保持は `model_clip_move_tests.rs`（`model_scale_tests::clip_move`）で時間に依らず検査する。
分割 ID のテイク・ノート・内容・トラック・配置・デバイス・パラメーターとの衝突は `clip_move_tests.rs` で検査する。
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

### ピアノロール

単一ノートのドラッグの吸着・Shift と最短長は `PianoGeometry.swift` の `PianoGesture.preview`、
解放時の計算と単一送信は `PianoRollView.swift` の `mouseUp` に置く。
回帰は `PianoDragTests.swift`（[ADR 0167](decisions/0167-piano-note-drag-snapping.md)）。

ピアノロールの進行中の操作は `PianoInteraction.swift` の列挙型が 1 つだけ持ち、ドラッグの遷移と取消の範囲もここに置く。
新しい操作は case を足し、`PianoGridView` の `mouseDown` と `mouseUp` の `switch` へ加える。
回帰は `PianoInteractionTests.swift`（[ADR 0206](decisions/0206-piano-interaction-state.md)）。

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
モデルの追加時の ID 検査の遅延・候補の衝突と未使用時の互換・時間に依らない探索回数は
`crates/echode-model/src/model_insert_tests.rs`（`model_scale_tests::insert`）で検査する。
自動配置の多重結合・リンクの一括付替えと整数境界・時間に依らない訪問と複製の回数は
`crates/echode-model/src/model_merge_tests.rs`（`model_scale_tests::merge`）で検査する。

単クリックの記憶長入力も同じ `PianoInsertion` を使い、長さと非永続の履歴枠は `NoteInputLength.swift` に置く。
`DocumentSession.recordEdit` が全モデル編集の枠をそろえ、ノート選択・追加・長さ変更だけが長さを更新する。
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
モデルの削除の所属探索・最後のノートの自動配置除去・固定リンクの保持と、時間に依らない費用の回帰は
`crates/echode-model/src/model_delete_tests.rs`（`model_scale_tests::delete`）に置く。
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

### アレンジ表示・描画面・ドロップ

アレンジの音高表示の音域と縦変換は `crates/echode-draw/src/query.rs` の `PitchRange`・`PitchLayout` に置く。
製品フレームと VoiceOver 用矩形の回帰は `ArrangePitchTests.swift`、可視列は `tests/arrange_pitch.rs` を使う。
問い合わせだけの計測は `make spike-arrange-query`（[ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
製品と検証用のループへの進入判定は `PlaybackAudio::arm_loop` と `Transport::arm_loop`、回帰は `RulerLoopTests.swift` と
`block_size.rs` の `contract_loops_enter_from_before_and_bypass_from_the_end_under_rtsan`（[ADR 0156](decisions/0156-ruler-loop-range-and-entry.md)）。

描画面の左上固定と drawable サイズの主スレッド更新は `CanvasSurface.swift`、対応する製品 View の回帰は `CanvasResizeTests.swift` に置く
（[ADR 0144](decisions/0144-unscaled-canvas-resizing.md)）。

出力の応答を位置・試聴・状態に分ける型は `Audio.swift`、部分ごとの書込みは `DocumentSession.receive`、回帰は `AudioReplyObservationTests.swift` に置く。
1 フレーム 1 回の文字の用意は `ArrangeLabels.swift`・`ArrangePainter.swift`、回帰は `ArrangeLabelCacheTests.swift`。
主スレッドと描画の計測は `MainThreadProbe.swift`（`make spike-main-thread`、[ADR 0289](decisions/0289-split-audio-replies-and-one-label-pass.md)）。

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
