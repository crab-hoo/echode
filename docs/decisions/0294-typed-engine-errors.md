# ADR 0294: エンジンのエラーを列挙型にし、エラーを文に変える `map_err` を拒否する

- 日付: 2026-11-09
- 状態: 採用
- 対象: [統合表](../reviews/2026-11-08-open-items-register.md) の M1（[横断監査](../reviews/2026-11-02-cross-cutting-audit.md#xca-16-文字列のエラーspg-11-の見直し) の XCA-16、SPG-11・DEV-7）のうち `echode-engine`。
- 方針: [ADR 0293](0293-typed-helper-boundary-errors.md) の「次の境界は `echode-engine`。型になった `SessionError` が最初に文字列へ戻る所だから」。
- 関連: [ADR 0291](0291-typed-audio-io-errors.md)（型の置き方と再導入の検査）、[ADR 0199](0199-owned-helper-control-requests.md)（ヘルパーの時間切れは結果不明）。

## 問題

- 開始時の HEAD は `c7994bc`。`echode-engine` の製品コードには `Result<_, String>` が 5 か所あった（`scripts/check_typed_errors.py` の数え方）。
  既定の出力の Workgroup を開く `IoWorkgroup::open` と、`spike` の機能でだけ作る Phase 0 の計測（`device_measurement.rs`）である。
- 数に出ない本体は `PlaybackError::Unsupported(String)` だった。製品の 13 か所（engine 12、audio-io 1）がこれを作り、
  うち 8 か所は、ヘルパーの起動・状態の復元・対応の準備の `SessionError`、共有領域の `RegionError`、Workgroup の `WorkgroupError`、
  スレッドの起動の `io::Error` を `error.to_string()` にして入れていた。
  ADR 0293 で型にした busy・ヘルパーの時間切れ（結果不明）は、エンジンを通ると文面に戻り、呼び出し側は分岐できなかった。
- 残りの 5 か所は固定の文（音声クリップのチャンネル数、バスの対応、プラグインの未起動、再生中のキャッシュ、ホストなしの書出し）で、
  別々の原因が 1 つの変種に入っていた。テストは `reason.contains("device workgroup or period is invalid")` のように文面で確かめていた。
- `check_typed_errors.py` は `Result<_, String>` だけを見るので、この形（列挙型の変種に文を入れる）を止められなかった。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | `Unsupported` を原因ごとの変種に分け、元のエラーは型のまま持つ。`SessionError` などの `Clone` でない型は `Arc` で包む | 採る |
| 2 | `Unsupported(String)` を残し、`Unsupported(Box<dyn Error>)` にする | 採らない。呼び出し側は `downcast` で探すことになり、変種を見て分岐できない |
| 3 | `PlaybackError` を `PartialEq` のまま保つため、`SessionError` に `PartialEq` を足す | 採らない。`io::Error`・`ExitStatus` を持ち、等しさの意味が決まらない。比較はテストの 12 か所だけで、`matches!` で足りる |
| 4 | 計測の失敗は `spike` 専用なので文字列のまま残し、検査の対象から外す | 採らない。crate 単位で検査を掛けられなくなる。計測の型は小さい |
| 5 | 検査に「`map_err` の中でエラーを文にしない」規則を足す | 採る。今回の 8 か所はこの形で、型付けを終えた 4 crate に該当は残っていない |

## 決定

### 型

`crates/echode-engine/src/error.rs` に `PlaybackError` を移し、`DeviceError` を置く。どちらも crate のルートから公開する。

| 型 | 変種 |
|---|---|
| `PlaybackError`（足した分） | `ClipChannels`、`BusMapping { device, clap_id }`、`Spawn { device, clap_id, error }`、`Helper { device, error }`、`FanIn`、`Workgroup`、`Thread`、`NoRunningPlugin`、`TransportBusy`、`NeedsPluginHost` |
| `DeviceError` | `Property { property, status }`（既定の出力・レート・バッファ・Workgroup の読込みの失敗）、`NoWorkgroup` |
| `MeasurementError`（`spike` だけ） | `Device`、レートの不正・不一致、周期・期間の表現の失敗、バッファの設定の失敗と不一致、並列スレッド数、IOProc の作成・開始 |

- `Spawn`・`Helper` は `Arc<SessionError>` を持つ。`Refused(Busy)`・`Refused(TimedOut(..))` が、エンジンを通っても型で届く。
- `Unsupported` を消した。`PlaybackError` から `PartialEq`・`Eq` を外し、テストの比較は `matches!` にした。

### 文面

- 次の 3 つを除いて従来のままである。照合している所はない（Swift・テスト・文書を検索した）。
  - 対応の取消（`cancel_mapping`）の失敗: `plugin helper did not start` を、ヘルパーの失敗の文（`Helper`）にした。起動の失敗ではなかった。
  - 計測の後の読み直し: `read buffer: {status}` を、最初の読込みと同じ `buffer size: {status}` にまとめた。
  - それ以外は、`Display` が従来の `Unsupported` の文と同じ文を作る。

### 再導入の検査

- `scripts/check_typed_errors.py` の `TYPED_CRATES` に `echode-engine` を足した。
- 同じ検査に、`map_err` の中で受け取ったエラーを `.to_string()`・`format!` で文にする書き方の拒否を足した（`turns an error into text`）。
  型付けを終えた crate では、文は表示の境界の `Display` で作る。
- 否定対照: HEAD の `echode-engine` に当てると 22 か所（`Result<_, String>` 5、文にする `map_err` 17）を拒否し、変更後は 0 である。
  ほかの 3 crate は HEAD でも新しい規則に当たらなかった。

## 回帰

- `hosted_playback.rs`: 壊れたバンドルの起動の失敗は `PlaybackError::Spawn` で、デバイスの ID と CLAP ID を持ち、文面は `device {id} ({clap_id}): {原因}` のままである。
  Workgroup を渡せない 2 つのテストは、文面の照合をやめて `PlaybackError::Workgroup(WorkgroupError::InvalidDevice)` を確かめる。
- `device_params.rs`: 実ヘルパーへ長さの違う状態を読ませると、`PlaybackError::Helper` がそのデバイスと `SessionError::Refused(Refusal::Other(_))` を持って届き、音は前のまま鳴る。
  ヘルパーのないデバイスは `NoRunningPlugin` である（`background_graph.rs` も同じ）。
- `render_tests.rs` はホストなしの書出しを `NeedsPluginHost`、audio-io の `session_tests.rs` は未起動のプラグインを `NoRunningPlugin` で確かめる。
- `scripts/tests/test_typed_errors.py`: 文にする `map_err` の 3 つの形を拒否し、変種への受け渡し・`Arc` での包み・注釈・`map_err` の外の `.to_string()` は通す。
- 製品の挙動は、上の文面のほかは変えていない。

## 残る範囲

- ほかの crate の `Result<_, String>` は、括弧を数えて 56 か所（開始時 61）。1 行の grep（`Result<.*, String>`、`tests` を含むパスを除く）では 62 → 57 である。
  多い順に `echode-history` 32、`echode-plugin-host` 20、`echode-ffi` 3、`echode-model` 1。
- 文にする `map_err` は、型付けの済んでいない crate に 200 か所ある（`echode-history` 131、`echode-plugin-host` 34、`echode-cli` 20、`echode-api` 12、`echode-ffi` 2、`echode-model` 1）。
  CLI・API の文は利用者への報告で、境界の文として残るものを含む。
- エンジンの中は、状態の復元が失敗するとどの原因でもグラフを作り直す（`restore_states`）。busy・時間切れを分けて扱う変更は、測って別に決める。
- 次の境界は `echode-plugin-host` とする。ヘルパーの側でプラグイン自身の失敗（状態の保存・読込み・ウィンドウ・走査）が文のまま `Refusal::Other` になる所で、
  `echode-history` より件数は少ないが、ADR 0293 の残りと同じ通信路の型を完成させる。
