# ADR 0292: 文書の手番のエラーを列挙型にする

- 日付: 2026-11-09
- 状態: 採用
- 対象: [統合表](../reviews/2026-11-08-open-items-register.md) の M1（[横断監査](../reviews/2026-11-02-cross-cutting-audit.md#xca-16-文字列のエラーspg-11-の見直し) の XCA-16、SPG-11・DEV-7）のうち `echode-document`。
- 方針: [ADR 0291](0291-typed-audio-io-errors.md) の「次の境界は `echode-document` で、ヘルパーの時間切れ（SPG-01）の区別と合わせて行う」。
- 関連: [ADR 0199](0199-owned-helper-control-requests.md)（ヘルパーの時間切れは結果不明）、[ADR 0276](0276-priority-order-and-delegated-decisions.md)（同じ規則を層ごとに別の論理で実装しない）。

## 問題

- 開始時の HEAD は `1c9e544`。`echode-document` の製品コードには `Result<_, String>` が 22 か所あった（`scripts/check_typed_errors.py` の数え方）。
- 手番・デバイスレーン・完了の `Ticket` が、どの失敗も同じ `String` で運んでいた。
  ヘルパーの時間切れ（`SessionError::Timeout`、結果不明）とヘルパーの拒否、デバイスの不在・ミキサー・オフライン、JSON の不正、
  履歴の拒否、出力の失敗、文書の停止・panic が、文面でしか分からなかった。
- ADR 0291 で `AudioError` にした出力の失敗も、この crate の 4 か所で文字列に戻っていた。
- テストも文面で確かめていた（`contains("mixer")`、`STOPPED` の文字列）。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | crate に 1 つの `DocumentError` を置き、包む原因の型（`HostError`・`PackageError`・`AudioError`・`SessionError`）をそのまま持つ | 採る |
| 2 | ヘルパーの失敗を `Timeout`・`Refused`・`Down` などへ写し直す | 採らない。区別は `SessionError` が既に持つ。写し直すと、sandbox の変種が増えるたびに 2 か所を直すことになる |
| 3 | `outcome_unknown()` のような判定の関数を置く | 採らない。製品の呼出し側がまだなく、使われない API になる。テストと将来の呼出し側は変種で照合する |
| 4 | C ABI に時間切れの状態コードを足す | 今は採らない。アプリは原因で表示を変えず、Swift の再試行の表示は文面で足りる。DEV-7 の完了の条件（ADR 0291 の残る範囲）と合わせて決める |

## 決定

### 型

`crates/echode-document/src/error.rs` に置き、crate のルートから `DocumentError` と `Rejected` を公開する。

| 変種 | 原因 |
|---|---|
| `Rejected(Rejected)` | 要求を受け付けず、何も変えていない。空・UTF-8 でない編集、未知の操作・履歴の要求、書出しの設定・行き先の欠け、復旧の先端・基準の欠け、デバイスの不在・ミキサー・オフライン、空のパラメーター変更 |
| `Malformed(serde_json::Error)` | 要求の JSON が、その操作として読めない |
| `History(HostError)` | 履歴が編集・Undo・Redo・復元・復旧の同意・版の一覧を拒否した、または失敗した |
| `Package(PackageError)` | パッケージを開けない、復旧の同意を待っている、物・書出し・設定を書けない |
| `Audio(AudioError)` | セッションが要求を拒否した、または出力を開けなかった |
| `Helper(SessionError)` | ヘルパーとの往復が失敗した。`SessionError::Timeout` は結果不明で、再送しない（ADR 0199） |
| `Encode(serde_json::Error)` | 報告を JSON にできない。この crate 自身の失敗で、要求の不正と分ける |
| `Thread(io::Error)` | 文書のスレッドを起こせない |
| `Stopped` | 完了の前に文書が止まった。チケットの破棄と、閉じたレーン |
| `Panicked` | 手番が panic した。以後の要求を拒否する |
| `DeviceRequestPanicked` | デバイスの往復が panic した。レーンは後続を続ける |

- `From` は `Rejected`・`HostError`・`PackageError`・`AudioError`・`SessionError` に置く。JSON の 2 つの意味は、明示の `map_err` で分ける。
- デバイスの検査（`hosted`）は `Rejected` だけを返す。
- 完了の `Ticket` は `Result<V, DocumentError>` を受け取る。`DocumentService::open` も `DocumentError` を返す。
- 包む型（`HostError`・`SessionError`・`serde_json::Error`）が `Clone` でないので、`DocumentError` も `Clone` にしない。完了は 1 回で、複製は要らない。
- engine と audio-io は、`PluginControl` と並べて `SessionError` を再公開した。

### 文字列にする所

- C ABI の完了（`echode-ffi` の `document.rs` の `finish`）だけで文字列にする。文面は全変種で従来のままである。
  Swift・FFI・CLI は文面で分岐していない（検索した）。FFI のテストの `document thread stopped` の照合は、変えずに通る。
- FFI の `Document::open` も `DocumentError` を返す。失敗は C ABI では null になるだけである。`echode-ffi` の 7 か所は 3 か所になった。

### 再導入の検査

- `scripts/check_typed_errors.py` の `TYPED_CRATES` に `echode-document` を足した。
- 否定対照: HEAD の `echode-document` に当てると 22 か所を拒否し、変更後は 0 である。
- 必須契約 `document-worker-completion` の否定対照の置換元を、新しい行（`complete(Err(DocumentError::Stopped));`）に直した。

## 回帰

- `lane_tests.rs` の `a_failed_or_panicking_round_trip_completes_with_an_error`: 時間切れは `Helper(SessionError::Timeout)` のまま完了へ届き、
  ヘルパーの拒否（`Protocol`）と区別される。C ABI へ渡る文面はヘルパーの文のままである。panic は `DeviceRequestPanicked`。
- `device.rs`: 未知の欄は `Malformed`、ミキサーは `Rejected::Mixer`、消えたデバイスの記録は `Rejected::DeviceGone`。
  `empty_parameter_changes_are_refused_before_the_output_opens` を足した。CLAP デバイスへの空の変更は、出力を開かず、編集中にも数えず `Rejected::ParameterChanges` になる。
- `service_tests.rs`・`ticket.rs`・`lane_tests.rs`: 停止・panic・未知の操作を、文面でなく変種で確かめる。
- 製品の挙動は変えていない。FFI の既存のテストは変えずに通る。

## 残る範囲

- 実ヘルパーの時間切れを文書のデバイスレーンまで通す回帰はない。実ヘルパーの時間切れは `echode-plugin-host` の
  `audit_spg01_a_timed_out_save_does_not_replace_the_next_restore_bytes` が持ち、文書の側は `From<SessionError>` の 1 経路で `Helper` に入る。
- ヘルパーの busy の拒否は、ヘルパーの文（`plugin main thread is busy with an earlier request`）が `SessionError::Protocol` に入り、型では分からない。
  sandbox と plugin-host の境界の作業で扱う。
- ほかの crate の残りは、括弧を数えて 85 か所（開始時 111）。1 行の grep（`Result<.*, String>`、`tests` を含むパスを除く）では 112 → 86 である。
  多い順に `echode-history` 32、`echode-plugin-host` 28、`echode-sandbox` 15、`echode-engine` 6、`echode-ffi` 3、`echode-model` 1。
- 次の境界は `echode-sandbox` とする。時間切れ・busy・ヘルパーの終了の区別が、engine の `PlaybackError::Unsupported(String)` と、この crate の `Helper` へ届く元だからである。
