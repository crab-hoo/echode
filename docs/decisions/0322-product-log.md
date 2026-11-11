# ADR 0322: 製品のログを `log` の窓口と一つのロガーに置き、オーディオスレッドの出来事はリングで制御スレッドへ渡す

日付: 2026-11-12
状態: 採用。実アプリでのファイルと統合ログの確認は UC-66 が未確認。
関連: 統合表の M4（[DEV-8](../reviews/2026-10-08-audit.md#dev-8-ログの仕組みを置く)・[XCA-19](../reviews/2026-11-02-cross-cutting-audit.md#xca-19-ログの仕組み)）、
[ADR 0238](0238-audio-callback-panics.md)、[ADR 0283](0283-output-time-callback-deadline.md)、[ADR 0286](0286-history-usage-diagnostics.md)、[ADR 0278](0278-audit-backlog-order.md)

## 問題

開始時の HEAD は `f64855a`。

- Rust に `tracing`・`log`、Swift に `os_log`・`Logger` がなかった。
  診断の出力は `eprintln!`（停止の診断の失敗、ヘルパーの起動の失敗）と `NSLog` 2 か所で、GUI のアプリでは誰も読めない。
- 残る記録は panic（`panic.ndjson`、ADR 0238）と試聴の要求（`audition.ndjson`）だけだった。利用者の環境で起きた出来事の順序は残らない。
- DEV-8 の完了の条件は「エンジンの締め切りの超過が、ノードの名前付きで記録される」。
  取りこぼしは `plugin_missed_blocks` の総数にしか残らず、どのデバイスかは書出しの失敗（最初の 1 件）でしか分からなかった。
- XCA-19 の修正方針は「製品の経路に `os_log` へつながる最小の記録を置く。オーディオスレッドからは書かない」。
- ADR 0286 の履歴の使用量は、JSON-RPC と MCP からしか読めなかった（「GUI では見えない。M4 で接続する」）。
- 回転の規則が 2 つあった。Rust の panic の記録は「1 MiB 以上なら移す」、Swift の試聴の記録は「追記で上限を超えるなら先に移す」。

## 比べた案

| 論点 | 案 | 判定 |
|---|---|---|
| 窓口 | `tracing`（DEV-8 の提案） | 採らない。span を使う予定がなく、`tracing-core`・手続きマクロ・`tracing-subscriber` が加わる |
| | `log` 0.4.34 の `kv`（構造化の欄） | 採る。既にロックファイルにあり（cbindgen 経由）、手続きマクロがない。欄は JSON の型のまま残る |
| | 自前のマクロ | 採らない。依存の crate の記録を受けられず、定番の窓口に劣る |
| 出力先 | 統合ログだけ | 採らない。info は保存されず、この環境では `log show` が `Could not open local log store` で読めなかった |
| | ファイルだけ | 採らない。XCA-19 の Console・sysdiagnose での突き合わせ（coreaudiod など他の系と並べる）ができない |
| | 両方 | 採る。ファイルは `make debug-report` が確実に集め、統合ログは呼んだスレッドと時刻で Console に出る |
| 統合ログの呼出し | `oslog` 0.2.0（MIT、C の薄い shim、既定の機能を外せば依存なし） | 採る |
| | `tracing-oslog`（Zlib） | 採らない。`tracing` が前提 |
| | 自前の C shim、Swift の `Logger` へ戻す | 採らない。前者は手書きの宣言を増やす。後者は CLI と別スレッドに届かず、FFI の呼び戻しが要る |
| ファイルへの書込み | 呼んだスレッドで同期に書く | 採らない。文書・主スレッドがディスクで止まる（堅牢性） |
| | 有界の待ち行列と書込みスレッド。満杯は捨てて数える | 採る |
| オーディオスレッド | 固定長の記録を `rtrb` のリングで渡す（DEV-8） | 採る。順序とサンプル位置が残り、満杯は数として残る |
| | デバイスごとの原子的な計数を制御側が読む | 採らない。順序と位置が消え、ヘルパーの作り直しで計数の持ち主が替わる |
| 洪水の抑え | 1 件ごとに書く | 採らない。毎ブロック取りこぼすプラグインは 48 kHz・128 フレームで毎秒 375 行になる |
| | 出来事の続きを 1 つにまとめ、最初は即時、以後は 10 秒ごとに件数 | 採る |
| ヘルパーの stderr | 行ごとにログへ流す | 採らない。常に出力する第三者のプラグインがログを埋める |
| | 終了の時に末尾 2 KiB を残す | 採る。全文は従来どおり `HostExited` のエラーにある |

## 決定

### ロガー（`echode-log`）

- 製品のコードは `log` のマクロで書く。欄は `key = value`（数・真偽・文字列）、`key:% = value`（Display）、`key:? = value`（Debug）。
  `log` の版と機能（`std`・`kv`）はワークスペースの依存の 1 か所に置く。
- `echode_log::install(Options)` はプロセスで 1 回だけ有効で、C ABI（アプリ）と CLI が呼ぶ。ライブラリとテストは呼ばない。
  - アプリ: `~/Library/Logs/Echode/echode.ndjson` と統合ログ。CLI: 統合ログだけ（stderr は命令の報告のまま）。
  - 既定の水準は info。`ECHODE_LOG=debug` などで変える。
- 1 件は 1 行の JSON である: `time_ms`・`pid`・`level`・`target`・`thread`・`message`・`fields`。
  64 ビットを超える整数と非有限の小数は文字列にし、丸めた数や不正な JSON を作らない。
- 統合ログは呼んだスレッドで書く。サブシステムは `io.echode`、カテゴリは crate 名から `echode_` を除いたもの（`engine`・`audio_io`、アプリは `app`）。
  error は error、warn は default（保存される）、info は info、debug 以下は debug に対応させる。文は `%{public}s` で、欄は JSON で続ける。
- ファイルは有界の待ち行列（1,024 件）を通し、書込みスレッド（`echode-log`）がまとめて追記する。
  満杯で捨てた件数と書込みの失敗は、次に書けた時の `log records were lost` の 1 行に残す。`flush` は最大 2 秒待つ。
  アプリは終了の通知で `flush` を呼ぶ。異常終了では待ち行列の残りは失われ、panic は従来どおり同期の `panic.ndjson` に残る。
- 回転の規則は `echode_log::append_lines` の 1 つにした。「空でないファイルが追記で上限を超えるなら、先に `<name>.1.<ext>` へ移す」。
  上限は 1 MiB（`FILE_LIMIT`）。panic の記録と、Swift の試聴の記録（C ABI の `echode_log_append_lines`）も同じ関数を通る。
- `echode_log::testing::capture` は、テストが製品の経路の記録を読む入口である。同じバイナリのテストは、スレッド名（テスト名）か欄で自分の記録を選ぶ。

### オーディオスレッド（`echode-engine` の `audio_log`）

- コールバックは記録しない。固定長の `Note` をリング（1,024 件）へ入れ、満杯の分は数える。
  - `Plugin`: ブロックを終えなかったステージのデバイスと理由（処理の拒否・締め切り・送信の失敗）と位置。
    `BlockReport` は各ブロックの全ステージの取りこぼしを持つ。容量は制御スレッドが音声側のグラフを作る時にステージ数だけ確保し、音声側では伸ばさない。
  - `Late`: `render_live_gated_until` が、コールバックの渡した締め切り（ADR 0283）より後に戻ったブロック。`Instant::now` を 1 回読む。
- `PlaybackControl::service`（文書のスレッドが 20 ms 以内ごとに呼ぶ）がリングを空にし、自分の複製のプロジェクトからトラックとプラグイン（CLAP ID）を引いて書く。
  終了したヘルパーの締め切りの取りこぼしは `helper exited` と書く。その判定はヘルパーのロックを `try_lock` で取り、待たない。
  準備や状態の保存が持っている間は実行中と見なし、文書のスレッドをヘルパーの後ろで止めない。`PlaybackControl` の破棄で残りの件数も書く。
- 自動ミュート（トラックとマスター、ADR 0282）はコールバックが既に公開している状態なので、リングを通さず、制御スレッドが変化を書く。
- 出来事の続き（同じデバイス・同じ理由、遅れたブロック、捨てた記録）は、最初の 1 件をすぐ書き、以後は数えて 10 秒ごとに件数・位置の範囲・最悪の遅れを書く。
  10 秒何も来なければ続きを閉じ、次はまた名前付きで書く。

### 記録する所

| 所 | 水準 | 内容 |
|---|---|---|
| `echode_audio_io::session` | info / warn | 出力を開いた（機器名・UID・レート・バッファ・安全余白・チャンネル）、閉じた（取りこぼしの総数）、出力の停止・失敗（文と型付きの原因） |
| `echode_audio_io::trace` | warn | 停止の診断の書込みの失敗（従来の `eprintln!`） |
| `echode_engine::audio` | warn / info | 上のノード名付きの取りこぼし、遅れたブロック、自動ミュート、リングの満杯 |
| `echode_sandbox::session` | warn | ヘルパーの終了（PID・終了状態・CLAP ID・stderr の末尾） |
| `echode_plugin_host::catalog` | info / warn | 走査の拒否、答えのない走査（ADR 0285） |
| `echode_history::host` | info / warn / error | 文書を開いた（復旧の理由）、保存の失敗、耐久性が不明で編集を止めた、自動保存・圧縮の失敗、閉じた時の履歴の使用量（ADR 0286） |
| `echode_document::worker` | error | 文書の手番の panic |
| `app::arrange`・`app::piano` | error | Metal の描画器を作れなかった（従来の `NSLog`） |

- パッケージは名前（最後の要素）だけを書き、フォルダのパスは書かない。トラック名は書かず ID を書く。
- 履歴の使用量は文書を閉じる時に 1 回だけ数える。版の並びをたどる時間（1,000 段 × 128 トラックで 4.6 ms、ADR 0286）は編集を待たせない。

## 測定

2026-11-12、Apple M4・macOS 27.0、release（作業ツリーの外の計測用の crate）。

- `Instant::now` は 1 回 27.8 ns（1,000 万回の平均）。64 フレームのブロック（1.33 ms）の 0.002% である。
- 欄 4 つの 1 件を書く呼出し側の時間は 2.24 µs（統合ログとファイル、2 万件の平均）。書込みスレッドは 2 万件を捨てずに書いた。
  画面の更新ごとや音声の 1 ブロックごとに書く量ではない。

## 回帰と基盤

- `echode-log` の `lib_tests.rs`: 回転の境界（追記で超える直前に移す、上限より長い 1 回の書込み）、欄の型、`install` の 1 回性とファイルへの到達、
  書込みの失敗と捨てた件数の報告。
- `audio_log_tests.rs`: 続きの最初の即時の 1 行、10 秒の要約、静かになった後の再開、終了したヘルパー、閉じる時の要約、
  `ForbidAlloc` の中でのリングの満杯、自動ミュートの開始と終了。
- 必須契約 `engine-deadline-log`（`hosted_playback` の `deadline_log::contract_a_plugin_that_misses_the_engine_deadline_is_logged_by_its_node`）:
  実ヘルパーを保留させて締め切りを越えさせ、`service` の後にデバイス・トラック・CLAP ID・理由・位置の 1 行と遅れたブロックの 1 行を確かめる。
  反例（`count_misses` がリングへ入れない）は `one line names the missing node: []` で失敗することを確かめた。
- `logging_tests.rs`（C ABI）と Swift の `LogTests`: アプリの記録がファイルへ届くこと、引数の拒否、試聴の記録の回転。
- `make debug-report` は `echode.ndjson` と `echode.1.ndjson` も `app-logs/` へ写す（`test_diagnostics.py`）。
- `scripts/verification.json` の `bridge`・`audio`・`render`・`product_audio` の領域に `crates/echode-log/**` を足した。

## 残る範囲

- 実アプリで、ファイルの作成と Console の `subsystem:io.echode` の表示はユーザー確認待ち（UC-66）。自動検査は配布したアプリの起動経路を通らない。
- ヘルパー（別プロセス）は記録しない。理由は終了の時の stderr の末尾でだけ親に届く（ADR 0238 の残りと同じ）。
- 遅れたブロックは、どのノードが時間を使ったかを示さない。同じブロックの取りこぼしの行と位置で突き合わせる。
  プロセス内の処理（混合・素材）のノードごとの時間は測っていない。毎ブロックの計時を足す前に、要る場面を測ってから決める。
- 履歴の使用量は閉じる時にしか書かない。作業中の値が要れば、JSON-RPC の `history.usage` を使う。
