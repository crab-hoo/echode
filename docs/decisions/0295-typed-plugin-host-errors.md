# ADR 0295: ヘルパーのエラーとプラグイン自身の失敗を列挙型にする

- 日付: 2026-11-09
- 状態: 採用
- 対象: [統合表](../reviews/2026-11-08-open-items-register.md) の M1（[横断監査](../reviews/2026-11-02-cross-cutting-audit.md#xca-16-文字列のエラーspg-11-の見直し) の XCA-16、SPG-11・DEV-7）のうち `echode-plugin-host`。
- 方針: [ADR 0294](0294-typed-engine-errors.md) の「次の境界は `echode-plugin-host`。プラグイン自身の失敗が文のまま `Refusal::Other` になる所で、ADR 0293 の残りと同じ通信路の型を完成させる」。
- 関連: [ADR 0293](0293-typed-helper-boundary-errors.md)（拒否の種類と通信の形式）、[ADR 0285](0285-scan-timeouts-are-not-verdicts.md)（走査の判定と時間切れ）、[ADR 0291](0291-typed-audio-io-errors.md)（再導入の検査）。

## 問題

- 開始時の HEAD は `796eeb4`。`echode-plugin-host` の製品コードには、`scripts/check_typed_errors.py` の数え方で 54 か所あった。
  `Result<_, String>` が 20（状態・ウィンドウ・ポートの照会、走査の管、目録、フォルダの監視）、エラーを文にする `map_err` が 34（読込み・起動・ソケット）。
- `HostError::Plugin(String)` は、読込み・インスタンスの作成と起動・ソケット・Workgroup・処理の拒否を 1 つの文に入れていた。
- ヘルパーのメインスレッドの要求は、プラグインの状態の保存・読込み、ウィンドウ、ポートの失敗を文で返し、通信路では `Refusal::Other(文)` だった。
  ホストは、状態の拡張を持たないプラグインと、今回だけ保存を断ったプラグインを、文面でしか区別できなかった。
- `ScanError` の 3 つの段（判定・時間切れ・読めない）は型だったが、中身はどれも組み立て済みの文で、`io::Error`・`serde_json::Error`・終了状態を捨てていた。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | プラグイン自身の失敗を `echode-sandbox` の `PluginFailure` にし、ヘルパーの内部でも通信路でも同じ型を使う（`Refusal::Plugin`） | 採る |
| 2 | ヘルパーの中だけ列挙型にし、通信路へ出す所で `Refusal::Other(error.to_string())` にする | 採らない。境界で文に戻すと、ホストは引き続き文面でしか分けられない。同じ原因を 2 つの層で別に定義することにもなる |
| 3 | `Refusal::Other` をなくし、読込みの失敗も型で送る | 今は採らない。読込みの失敗の多くは dlopen・clack の文（`libloading::Error`）で、型にしても中身は文になる。ホストはこれで分岐しない |
| 4 | プラグインの失敗を領域ごとの入れ子の型（状態・窓・ポート）にする | 採らない。通信の形式が 3 段になる。分岐の必要が出たら、平らな型に判定のメソッドを足せば足りる |
| 5 | `ScanError` の 3 段を保ち、各段の中身を型にする | 採る。キャッシュの判定（`is_verdict`）と遅い走査の再試行（ADR 0285）は段で決まり、変えない |

- 制御の通信路はアプリに同梱する同じ版のヘルパーとだけ話す（ADR 0293）。新しい種類に互換の層は要らない。

## 決定

### 型

| 型 | 置き場所 | 変種 |
|---|---|---|
| `PluginFailure` | `echode-sandbox` の `refusal.rs` | 状態（`NoState`・`NoStateSave`・`NoStateLoad`・`SaveRefused`・`LoadRefused`）、`CapacityRefused`、`NoParameterFlush`、窓（`NoWindow`・`NoCocoaWindow`・`NoWindowCreate`・`WindowRefused`・`NoEmbedding`・`ParentRefused`、ヘルパー側の `PanelNeedsMainThread`・`PanelHasNoView`）、音声ポート（`NoAudioPorts`・`NoPortCount`・`NoPortInfo`・`PortUndescribed(AudioPort)`・`PortWithoutChannels(AudioPort)`・`NoPorts`）、ノートポート（4 つ） |
| `Refusal::Plugin(PluginFailure)` | 同上 | 要求は終わっており、結果不明ではない |
| `HostError` | `echode-plugin-host` の `lib.rs` | `Entry(PluginEntryError)`、`NoBinary`・`NoFactory`・`NoPlugin`（パス）、`NoPluginId`、`Instance(PluginInstanceError)`、`Plugin(PluginFailure)`、`StartRefused`、`ProcessRefused`、`Socket(io::Error)`、`Workgroup`、`RunLoopSource`、`NoRunLoop`、`PortsUnpublished`、`Panicked(スレッド名)`、`FrameCount` |
| `ScanError` | `scan.rs` | `Rejected(ScanFailure)`・`TimedOut(ScanProgress)`・`Unread(ScanUnread)` |
| `ScanFailure` | 同上 | `Failed { bundle, status, stderr }`、`TooLarge`、`NotText`、`Malformed(serde_json::Error)`、`Empty` |
| `ScanProgress`・`ChildState` | 同上 | 子の番号・状態（実行中・終了状態・`try_wait` の失敗）と、両方の管の読んだ量と開閉 |
| `ScanUnread` | 同上 | `Start`・`MissingStdout`・`MissingStderr`・`Pipe`（`io::Error`）・`Wait { bundle, error }` |
| `CatalogError` | 同上 | `MissingHost`・`Cache`・`Thread`（目録の更新と監視の開始） |
| `WatchError` | `watch.rs` | `NotUtf8`・`Create`・`Start` |

- ヘルパーのメインスレッドの要求は `Result<MainReply, Refusal>` を返す。状態・窓・ポートの関数は `PluginFailure` を返し、`From` で `Refusal::Plugin` になる。
- 起動時のポートの失敗（`PortShape::error`）と、起動時の `CapacityRefused` は `Refusal::Plugin` で `PortsRejected` に入る。
  ほかの起動の失敗（読込み・作成・起動）は、ローダーの文のまま `Refusal::Other` で送る（案 3）。
- 起動時の `CapacityRefused` は、前は親へ何も送らずに終わり、親はヘルパーの終了と stderr でしか知らなかった。ほかの 2 つの起動の失敗と同じく拒否を送ってから終える。
- `HostInfo` は固定の C 文字列から作る `new_from_cstring` にし、失敗しない。
- メインスレッドの呼出し番号は拒否の中の名前だけで、完了は呼出しそのもの（`Arc::ptr_eq`）で照合する。
  2^64 回の枯渇で拒否していた所は、番号を折り返すことにした。到達せず、折り返しても名前が重なるだけである。

### 通信の形式

- 種類 `7`（`PLUGIN`）の後に失敗の番号 1 バイト（0〜24）を置く。2 つのポートの種類（18・19）だけ、続けて側（0 は出力、1 は入力）と u32 LE の番号を置く。
- 読込みは書く形だけを受け取る。未知の番号、ポートの種類の欠けたポート、ほかの種類に付いたポート、2 以上の側は不正なデータである。
  fuzz の往復の性質（読んだ物を書くと同じ物に戻る）はそのまま成り立つ。fuzz の種に `PortsRejected`・`StateRejected`・`GuiRejected` の各 1 つを足した。

### 文面

- 次を除いて従来のままである。照合している所はない（Swift・テスト・文書を検索した）。
  - プラグインが読み込まれていないメインスレッドの要求: `plugin is not loaded` を `Refusal::NoPlugin`（`no plugin`）にした。`call_main` の前の確認で通常は届かない。
  - 走査の時間切れのうち、両方の管が閉じてから終了を待つ間のもの: `plugin scan timed out ({bundle})` を、管の量と子の状態を付けた形にした。管を読む間の時間切れと同じ形である。
  - `measure_in_process`（`spike` の計測だけ）: `no factory`・`no plugin` をパス付きの文に、clack の `Failed to start plugin processing` を `plugin refused to start processing` にした。
- `Display` が従来の文を作るので、目録・FFI・エンジンを通る文はほかに変わらない。FFI の目録（`catalog.rs`）は C ABI の境界で文にする。

### 再導入の検査

- `scripts/check_typed_errors.py` の `TYPED_CRATES` に `echode-plugin-host` を足した。
- 否定対照: HEAD の `echode-plugin-host` に当てると 54 か所（`Result<_, String>` 20、文にする `map_err` 34）を拒否し、変更後は 0 である。

## 回帰

- `refusal_tests.rs`（`echode-sandbox`）: 全 27 の失敗（ポートの種類は両側）が往復し、番号は重ならない。
  読込みが受け取る全番号は同じバイトに書き戻る。番号のない本体、ポートの欠け・余り、未知の番号と側は `InvalidData` である。表示の文は従来のままである。
- `device_params.rs`（実ヘルパー）: 長さの違う状態を読ませると、`PlaybackError::Helper` が `SessionError::Refused(Refusal::Plugin(PluginFailure::LoadRefused))` を持って届く。
  ADR 0294 の時は `Refusal::Other(_)` までしか確かめられなかった。表示は `plugin refused to load state` のままで、音は前のまま鳴る。
- `tests/scan.rs` の判定・時間切れ・文面の照合（終了状態・stderr・`stdout: N bytes`・`: running;`）は変更なしで通る。`live_tests.rs` は型の値で偽の走査を作る。
- 起動時の `CapacityRefused` の拒否には自動の回帰がない。この拡張を断る試験用のプラグインがない（実装は Sine Pad だけ）。送り方はほかの 2 つの起動の失敗と同じ関数である。

## 残る範囲

- ヘルパーが `Refusal::Other` を作るのは、ローダーの文の起動の失敗、設計上起きないメインスレッドの応答の食い違い（4 か所で作り、7 つの要求が使う）、無効なパラメーター ID の 3 つである。
  無効な ID はホストが有限性しか確かめないために届きうる（`u32::MAX`）。ホストで送る前に拒否する変更は別に決める。
- ほかの crate の `Result<_, String>` は、括弧を数えて 36 か所（開始時 56）。1 行の grep（`Result<.*, String>`、`tests` を含むパスを除く）では 57 → 37 である。
  多い順に `echode-history` 32、`echode-ffi` 3、`echode-model` 1。
- 文にする `map_err` は、型付けの済んでいない crate に 168 か所（開始時 200）。`echode-history` 131、`echode-cli` 20、`echode-api` 12、`echode-ffi` 4、`echode-model` 1。
  `echode-ffi` の 2 か所の増加は、目録の文を C ABI の境界で作るようにした分である。
- 次の境界は `echode-history` とする。残りの `Result<_, String>` の 32/36、文にする `map_err` の 131/168 を持つ。
