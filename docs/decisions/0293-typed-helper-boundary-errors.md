# ADR 0293: ヘルパーとの境界のエラーと拒否を列挙型にする

- 日付: 2026-11-09
- 状態: 採用
- 対象: [統合表](../reviews/2026-11-08-open-items-register.md) の M1（[横断監査](../reviews/2026-11-02-cross-cutting-audit.md#xca-16-文字列のエラーspg-11-の見直し) の XCA-16、SPG-11・DEV-7）のうち `echode-sandbox`。
- 方針: [ADR 0292](0292-typed-document-errors.md) の「次の境界は `echode-sandbox`。時間切れ・busy・ヘルパーの終了の区別が届く元だから」。
- 関連: [ADR 0199](0199-owned-helper-control-requests.md)（ヘルパーの時間切れは結果不明、処理中の要求は busy で拒否）、[ADR 0291](0291-typed-audio-io-errors.md)（型の置き方と再導入の検査）。

## 問題

- 開始時の HEAD は `8d538be`。`echode-sandbox` の製品コードには `Result<_, String>` が 15 か所あった（`scripts/check_typed_errors.py` の数え方）。
  14 か所は Workgroup の保持・受け渡し・参加と、ヘルパーの起動（`posix_spawn` の各段）で、残りは背景保存の拒否の理由だった。
- `SessionError` は `Workgroup(String)` と `Protocol(String)` を持ち、`Protocol` に次の 3 種類が同じ文字列で入っていた。
  ヘルパーの拒否（`*Rejected` の本文）、手順の違反（予期しない応答・ポートのチャンネル数のあふれ）、こちら側の拒否（背景保存の重複・有限でない値）。
  起動の失敗は `Io(io::Error::other(文))`、起動中の失敗に stderr を足す時は元の型を捨てて `Protocol(format!(..))` にしていた。
- ヘルパーは拒否を UTF-8 の文だけで送っていた。busy（ADR 0199 の処理中の拒否）も、ヘルパー自身のメインスレッドの待ちの時間切れ（`timed out; outcome unknown`）も、
  プラグインの拒否と同じ `Protocol(文)` になり、ホストは結果不明の応答を確定の拒否と区別できなかった。
- 実ヘルパーで確かめると、SPG-01 の遅い保存では、ホストの 2 秒の待ちよりヘルパーの 2 秒の待ちが先に終わる。
  `capture_state` の失敗は `SessionError::Timeout` ではなく `Protocol("plugin main request 1 timed out; outcome unknown")` だった。
  ADR 0292 が「結果不明は `SessionError::Timeout`」とした区別は、この経路の大半を取りこぼしていた。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | 拒否を種類の 1 バイトと種類ごとの中身で送り、ホストとヘルパーが同じ `Refusal` を使う | 採る |
| 2 | 拒否は文のまま送り、ホストで busy・時間切れの文面を照合して型にする | 採らない。文面で分岐しないための作業で、照合を新しく作ることになる。ヘルパーの文が変わると黙って外れる |
| 3 | ヘルパーの時間切れ・停止を `SessionError::Timeout` に写す | 採らない。ホストの待ちの時間切れ（ヘルパーが答えない）と、ヘルパーが答えた上での時間切れは診断で分けたい。結果不明という扱いは文書の注で揃える |
| 4 | `Refusal` を `Busy`・`Other(String)` の 2 つだけにする | 採らない。時間切れと停止は結果不明で、確定の拒否と同じ型に入れると問題の取りこぼしが残る |
| 5 | プラグイン自身の失敗の文（状態の保存・読込み・ウィンドウ）まで変種にする | 今は採らない。`echode-plugin-host` の境界の作業で行う。今回は `Refusal::Other` に入れる |

- 制御の通信路はアプリに同梱する同じ版のヘルパーとだけ話し、版の交渉はない。形式の変更に互換の層は要らない。

## 決定

### 型

`crates/echode-sandbox/src/error.rs` に `SessionError` を移し、新しい型を置く。`Refusal` は `refusal.rs` に通信の形式と一緒に置く。どれも crate のルートから公開する。

| 型 | 変種 |
|---|---|
| `Refusal` | `Busy`（前の呼出しが動いていて始めていない）、`NoPlugin`、`TimedOut(Pending)`・`Stopped(Pending)`（結果不明）、`Other(String)`（ヘルパーの文） |
| `Pending` | `MainCall(番号)`（メインスレッドの呼出し）、`ParameterChanges`（処理スレッドへ渡したパラメーター） |
| `WorkgroupError` | 機器・受け渡しの値の不正、null の Workgroup・ポート、名前の nul、Mach・Workgroup の呼出しの状態（`WorkgroupCall`）、登録・受信・作成・スレッドの方針の失敗 |
| `SpawnError` | パス・引数・環境の nul、パイプ、`posix_spawn` の各段（`SpawnStep`）の errno、stderr のパイプの欠け |
| `ProtocolError` | 予期しない応答（待っていた応答を名前で持つ）、チャンネル数のあふれ、待ち行列の消失、要求番号の枯渇 |

`SessionError` の変種は次のとおりにした。

- `Workgroup(WorkgroupError)`・`Spawn(SpawnError)`: 文字列と `Io` の包みをやめた。
- `Refused(Refusal)`: ヘルパーの `*Rejected` の応答。`ChainRefused`・`RemapRefused` は本文のない拒否である。
- `SavePending`・`NonFiniteParameter`: 送る前にこちらで拒否した。
- `Protocol(ProtocolError)`: ヘルパーの手順の違反と、こちらの食い違い。
- `Launch { error, stderr }`: 起動中の失敗に stderr を足す。元の原因を `Box` で持つ。
- `RegionError::Protocol(String)` は 1 つの固定の文だけだったので `UnstorableEvent` にし、構築されていない `ProcessFailed(String)` を消した。
  音声スレッドの書込みの失敗の道で、文字列の確保がなくなった。

### 通信の形式

- `PortsRejected`・`StateRejected`・`ParamsRejected`・`GuiRejected` の本文は、種類の 1 バイトと中身である。
  `Other` は UTF-8 の文、`MainCall` は 8 バイトの番号、ほかは中身なし。
- 読込みは書く形だけを受け取る（未知の種類、余りのバイト、長さの違う番号、UTF-8 でない文は不正なデータ）。
  fuzz の往復の性質（読んだ物を書くと同じ物に戻る）がそのまま成り立つ。fuzz の種に全種類を足した。

### ヘルパーの側

- `MainRequests::begin` は処理中なら `Refusal::Busy` を返す。`call_main` は `NoPlugin`・`Stopped(MainCall)`・`TimedOut(MainCall)` を返す。
- パラメーターの適用は、busy の事前の拒否を `Busy`、処理スレッドの待ちの時間切れを `TimedOut(ParameterChanges)`、停止を `Stopped(ParameterChanges)` にした。
  時間切れは、処理スレッドが受け取る前に取り下げた場合も結果不明として返す。取り下げの前に受け取られていれば適用されうるので、安全側に揃えた。
- プラグイン自身の失敗（`perform_main_request` の文）は `Refusal::Other` に入れる。

### 文字列にする所と文面

- 表示の境界だけで文字列にする（engine の `PlaybackError::Unsupported`、`echode-plugin-host` の `HostError::Plugin`、FFI の完了）。
- 文面は次の 3 つを除いて従来のままである。照合している所はない（Swift・テスト・文書を検索した）。
  - パラメーターの時間切れ: `parameter changes timed out` に `; outcome unknown` を足した。結果不明であることを文面でも示す。
  - 起動時の周期 0: `workgroup period is zero` を、同じ意味の `workgroup port or period is invalid` にまとめた。
  - 待ち行列の消失: `queued notice disappeared` を `queued control message disappeared` にまとめた。到達しない内部の食い違いである。
- engine の `PluginWorkgroup` は `Result<WorkgroupHandoff, WorkgroupError>` を返す。audio-io の `WorkgroupFault::Retain` は `WorkgroupError` を持つ。
  engine と audio-io は `Refusal`・`Pending`・`WorkgroupError` を再公開した。

### 再導入の検査

- `scripts/check_typed_errors.py` の `TYPED_CRATES` に `echode-sandbox` を足した。
- 否定対照: HEAD の `echode-sandbox` に当てると 15 か所を拒否し、変更後は 0 である。

## 回帰

- `refusal_tests.rs`: 全種類が往復し、busy の文面を持つ `Other` は `Busy` にならない。不正な本文は `InvalidData`。
- `session_operations_tests.rs` の `public_control_rejections_preserve_the_last_applied_state`: 7 つの操作の拒否に全種類を割り当て、どれも `SessionError::Refused` の同じ種類で届くこと、文面が `Refusal` の表示と同じことを確かめる。
  有限でない値は `NonFiniteParameter` で、通信路へ出ない。
- `main_requests_tests.rs`: 処理中の 2 つ目の要求は `Refusal::Busy`。
- 必須契約 `plugin-main-timeout-state`（`audit_spg01_a_timed_out_save_does_not_replace_the_next_restore_bytes`）を強めた。
  実ヘルパーの遅い保存は `Timeout` か `Refused(TimedOut(MainCall))`（結果不明）でだけ失敗し、その後ろの復元は `Refused(Busy)` で拒否される。
  確かめた実行では、保存は `Refused(TimedOut(MainCall(1)))`、復元は `Refused(Busy)` だった。
- `lane_tests.rs`（文書）: busy の拒否は `Helper(SessionError::Refused(Refusal::Busy))` で完了へ届き、文面は従来のままである。
- 製品の挙動は、上の 3 つの文面のほかは変えていない。

## 残る範囲

- 背景保存（`poll_state_save`）は、busy・時間切れで拒否された保存を従来どおり捨て、次の変更の通知まで頼み直さない。
  型で分けられるようになったが、`service` は制御スレッドから頻繁に呼ばれ、busy の間に毎回頼み直すと要求が続く。間隔を決めて頼み直す変更は、別の作業で測って決める。
- engine は `SessionError` を `PlaybackError::Unsupported(String)` の文面にまとめ、時間切れ・busy・起動の失敗を分けない（`plugins.rs` の起動・状態の復元・対応の準備）。
- `echode-plugin-host` の内部の失敗（状態の保存・読込み・ウィンドウ・走査）は文のままで、通信路では `Refusal::Other` になる。
- ほかの crate の残りは、括弧を数えて 61 か所（開始時 85）。1 行の grep（`Result<.*, String>`、`tests` を含むパスを除く）では 86 → 62 である。
  多い順に `echode-history` 32、`echode-plugin-host` 20、`echode-engine` 5、`echode-ffi` 3、`echode-model` 1。
- 次の境界は `echode-engine` とする。型になった `SessionError` が、文書・アプリへ届く前に最初に文字列へ戻る所だからである。
