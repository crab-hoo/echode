# ADR 0291: 出力・試聴のエラーを列挙型にする

- 日付: 2026-11-09
- 状態: 採用
- 対象: [統合表](../reviews/2026-11-08-open-items-register.md) の M1（[横断監査](../reviews/2026-11-02-cross-cutting-audit.md#xca-16-文字列のエラーspg-11-の見直し) の XCA-16、SPG-11・DEV-7）のうち `echode-audio-io`。
- 方針: [ADR 0278](0278-audit-backlog-order.md) の「M1・M2 は crate の境界ごとに進め、`echode-audio-io` から始める」。
- 関連: [ADR 0234](0234-typed-journal-durability.md)（保存の不確定の型）、[ADR 0284](0284-audio-path-panic-lints.md)（音声経路の lint の検査）。

## 問題

- 開始時の HEAD は `f0e91bd`。`echode-audio-io` の製品コードには `Result<_, String>` が 68 か所（統合表と同じ 1 行の grep）あった。
  複数行・入れ子の型も数えると 77 か所である（下の検査の数え方）。
- 原因の区別が型に残らなかった。要求の拒否（負の位置・不正なループ）、出力なし、CoreAudio の状態コード、出力の停止（機器の変化・コールバックの panic）、
  エンジンの失敗、パッケージの読み書きが、どれも同じ `String` だった。
- セッションは最後の失敗を文字列で持ち、出力が開かなかった時のデバイス要求へそのまま返していた。
  呼出し側は原因で分けられず、分けるには文面の照合が要った（XCA-16 が `host.rs` で見つけた型の誤り）。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | crate に 1 つの `AudioError` を置き、原因の群ごとに入れ子の列挙型を持たせる | 採る |
| 2 | 層ごとの型（`HardwareError`・`StreamError`・`SessionError`）を重ねる | 採らない。セッションの失敗は流れ・機器の失敗を包むだけで、呼出し側が分ける原因は同じ数個である。`From` の連鎖と包み直しが増える |
| 3 | `thiserror` の derive で表示を作る | 採らない。engine・plugin-host・history の境界の型（`PlaybackError`・`HostError`・`PackageError`）は手書きの `Display` で揃っている。直接の依存と proc-macro のビルドを足すほどの量ではない（約 150 行） |
| 4 | `Box<dyn Error>` や `anyhow` | 採らない。原因で分岐できず、文字列と同じく照合が要る |

## 決定

### 型

`crates/echode-audio-io/src/error.rs` に置き、crate のルートから公開する。

| 変種 | 原因 |
|---|---|
| `Rejected(Rejected)` | 要求が不正で、何も変えていない。ミキサーのプレビュー、負の位置・開始位置、ループ、動くプラグインがない、試聴の音量 |
| `NoOutputDevice` | 使える出力がない |
| `CoreAudio { call, status }` | CoreAudio の呼出しが失敗した。`call` は属性の読込み・通知の登録・IOProc の作成と開始（出力と試聴） |
| `Hardware(HardwareFault)` | CoreAudio の答えが使えない。大きさ・一覧の変化・名前・チャンネル数・レート・形式 |
| `Workgroup { device, fault }` | 選んだ機器の Workgroup を用意できない。時計・周期・問合せ・保持 |
| `Stopped(Stopped)` | 開いた出力が自分で止まった。Workgroup の変化、機器の変化、コールバックの panic、ドライバーがコールバックを離さない |
| `Audition(AuditionFault)` | 試聴が鳴らない・止まった |
| `Playback(PlaybackError)` | エンジンが拒否した、または準備できなかった |
| `Package(Arc<PackageError>)` | パッケージの物・設定・音声ファイルを読み書きできない |
| `NoPackage` | 記録したプラグインの状態があるのに、セッションにパッケージがない |
| `Thread(ThreadFault)` | スレッドを起こせない、準備のスレッドが結果を送らずに終わった・panic した |
| `Internal(Internal)` | セッション自身の手順の食い違い |

- セッションは最後の失敗を `Option<AudioError>` で持つ。開けなかった出力を待っていたデバイス要求は、その原因を型のまま受け取る。
- `AudioError` は `Clone`。`PackageError` と `std::io::Error` は複製できないので `Arc` で持つ。`source()` は包んだ原因を返す。
- engine の `restore_recorded_states` は、記録の読込みの失敗を区別せず作り直しに回すだけなので、エラーの型を総称にした。
- 検査用の `HeadlessOutput::fail_next` は文字列でなく `AudioError` を受け取る。

### 文字列にする所

- 表示の境界だけで文字列にする。`AudioState.error`、`AuditionReport` の `error`・`output_error`（アプリへ JSON で渡る）、
  FFI の波形の概観の JSON、まだ文字列を返す `echode-document` の 4 か所の呼出しである。
- 文面は従来のままにした。アプリ・API・テストは文面で分岐していない（Swift・テスト・文書を検索した）。
  変えたのは利用者に届きにくい次の 3 つだけで、どれも照合している所はない。
  - 位置の変換のあふれ: `playback time overflow` を engine の同じ意味の `playback position overflowed` にした。
  - Workgroup の周期・時計: `output device <id> workgroup: ...` の形に揃えた。
  - 属性の大きさのあふれ・一覧の増加: それぞれ `CoreAudio property size changed`・`invalid CoreAudio list size` にまとめた。

### 再導入の検査

- `scripts/check_typed_errors.py` は、`TYPED_CRATES` の crate の製品コードに `Result<_, String>` がないことを確かめる。
  署名・チャネル・クロージャの型を、複数行・入れ子・末尾のカンマも含めて括弧を数えて見る。`*_tests.rs`、`tests/`、末尾の `#[cfg(test)] mod tests` は除く。
- `make check`（`check-base`）と `check-portable` で動く。`scripts/tests/test_typed_errors.py` が見つける形と除く形を固定する。
- 否定対照: HEAD の `echode-audio-io` に当てると 77 か所を拒否し、変更後は 0 である。

## 回帰

- `session_tests.rs`: 負の位置・開始位置・不正なループ・ミキサーのプレビューは `Rejected` の各変種で拒否する。
  `device_access_without_a_running_plugin_does_not_open_hardware` は `Rejected::NoRunningPlugin` を確かめる。
  `a_failed_opening_reports_its_error_and_is_not_retried_by_service` は、準備の失敗（`PlaybackError::Plugin`）を状態では文面で、デバイス要求では型のまま返すことを確かめる。
- `stream_tests.rs`: コールバックの panic は `Stopped::CallbackPanicked` で、理由に場所と文を持ち、文面は従来の前置きのままである。
- `audition_tests.rs`: 範囲外の試聴の音量は `Rejected::AuditionGain`。`workgroup.rs`: 問合せの失敗は原因の CoreAudio の状態を持ち、保持の失敗と区別する。
- 製品の挙動は変えていない。既存の出力・試聴・文書・FFI・CLI のテストは変えずに通る。

## 残る範囲

- ほかの crate の `Result<_, String>` は、統合表の 1 行の grep で 101 か所（開始時 169）、括弧を数えると 111 か所が残る。
  多い順に `echode-history` 32、`echode-plugin-host` 28、`echode-document` 22、`echode-sandbox` 15、`echode-ffi` 7、`echode-engine` 6、`echode-model` 1。
- `echode-document` は `AudioError` を受けた所で文字列にする。次の境界の作業で、ヘルパーの時間切れ（SPG-01）と合わせて型にする。
- アプリは文面だけを受け取り、原因で表示を変えない。DEV-7 の完了の条件（UI が使う境界のエラーが列挙型）は、アプリが原因で表示を分ける時に FFI の形と合わせて決める。
- engine は、ヘルパーの起動・状態の読込み・対応の準備の失敗を `PlaybackError::Unsupported(String)` の文面にまとめ、
  sandbox の時間切れ（`WaitError::Timeout`）を区別しない（`plugins.rs` の `prepare_mapping`・`restore_state` の包み）。engine の境界の作業で扱う。
- `echode-sandbox` の Workgroup の保持の失敗は文字列のまま `WorkgroupFault::Retain` に入る。sandbox の境界の作業で型にする。
