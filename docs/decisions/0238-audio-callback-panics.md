# ADR 0238: オーディオコールバックの panic を捕まえ、理由を残す

- 日付: 2026-11-03
- 状態: 採用
- 関連: [ADR 0047](0047-product-audio.md)、[ADR 0135](0135-sample-audition.md)、[横断監査](../reviews/2026-11-02-cross-cutting-audit.md)の XCA-14

## 背景

開始時の HEAD は `5565b4d`。

XCA-14 は読解から次を挙げた。
出力と試聴の `render` は `extern "C-unwind"` で `catch_unwind` がなく、release も `panic = unwind` なので、panic は CoreAudio のフレームへ伝わる。
FFI の関数は `extern "C"` で、panic は即時に異常終了する。panic フックがなく、GUI では標準エラーが見えないので理由が残らない。

作業中に、デバッグのビルドではさらに理由が消えることが分かった。
`GuardAlloc` は `ForbidAlloc` の間の確保をすべて異常終了にする。panic も文の整形と payload の箱で確保するので、
コールバックの中の panic は、フックが文を受け取る前に SIGABRT で終わっていた。
対象はデバッグのテスト（`echode-engine`・`echode-audio-io`）、デバッグの CLI、デバッグのヘルパーである。アプリは `GuardAlloc` を入れていない。

## 決定

- **コールバックで止める**: 出力と試聴のコールバックは、ブロックの本体を `CallbackPanic::run`（`catch_unwind`）で包む。
  panic したブロックは出力を 0 で埋め、`failed` を立てる。以後のブロックは本体へ入らず無音を返す。
  途中まで書き換えたプレイヤーの状態を、もう一度使わないためである。
  payload はコールバックが持ち、制御スレッドが理由を読む時に解放する。オーディオスレッドでは解放しない。
- **フック**（`echode_audio_io::keep_panic_log`）: プロセスで 1 回だけ入れる。
  - コールバックの本体を実行中のスレッドでは、場所と文を、そのコールバックが前もって確保した 512 バイトの領域へ写すだけにする。
    ファイルへ書かず、標準エラーのロックを取らず、確保もしない。前のフックも呼ばない。長い文は文字の境界で切る。
  - 実行中かどうかの判別にスレッドローカルを使わない。macOS のスレッドローカルは、そのスレッドで初めて触れた時に dyld が確保する。
    HAL のスレッドで毎ブロック触れると、最初のブロックで確保が起き得る。
    代わりに、各コールバックが制御スレッドで領域を確保し、本体の実行中だけ自分の `pthread_self` をそこへ書く。
    フックは領域の連結リストをたどって一致を探す。領域は解放せず再利用するので、フックが読む間に消えない。
    リストは同時に生きているコールバックの数まで伸び、上限はない。
  - ほかのスレッドでは、フックが記録へ時刻・場所と文・スレッド名・バックトレースを追記し、前のフックを呼ぶ。
    `extern "C"` の FFI はこの後に異常終了するので、アプリではこの行が唯一の記録になる。
- **報告**: 出力の `service` は、`audio playback stopped because the audio callback panicked: <場所>: <文>` を返す。
  セッションは、バッファや機器の変化による失敗と同じく出力を止め、エラーを表示する。
  試聴は鳴っている要求を `the audition callback panicked: ...` で止め、出力を閉じる。機器の変化とは言わない。
  どちらも最初の問い合わせで記録へ 1 行を追記する。
- **ガード**: `GuardAlloc` は、panic 中のスレッドの確保を許す（`std::thread::panicking()`）。
  std は panic の計数を増やしてから文を整形し、フックを呼び、payload を箱に入れる。この順は std のソースで確かめた。
  それ以外の確保は従来どおり異常終了する。プラグインのヘルパーのガードも同じにした。
- **記録の場所**: アプリは `EchodeApp.init` から `ProjectController.keepPanicLog()` を呼び、`~/Library/Logs/Echode/panic.ndjson` へ書く。
  試聴の記録の隣で、1 MiB を超えると `panic.1.ndjson` へ移す。`make debug-report` が両方を `app-logs/` へ写す。
  CLI はファイルを持たず、コールバックの捕捉だけを入れる。エラーに場所が入り、ほかの panic は従来どおり標準エラーに出る。
  ライブラリやテストは、呼ばなければ何も書かない。テストは一時ディレクトリへ書く。

## 却下した案

- **release を `panic = "abort"` にする**: フックで理由は残るが、アプリ全体が落ちる。コールバックの panic は再生の停止で済む。
- **フックを全スレッドで同じにし、オーディオスレッドでもファイルへ書く**: 単純だが、オーディオスレッドでファイル I/O をする。
  前のフック（標準エラーのロック、`thread::current` の確保、バックトレースの取得）も走る。
- **スレッドローカルでコールバックの実行中を示す**: 上の確保の理由で採らない。
- **固定長の領域の表**: 出力の数に上限を設けない規則に合わせ、必要なだけ伸びる連結リストにした。

## 検証

- `stream::tests::a_panic_in_the_render_leaves_the_output_silent_and_reports_its_location`:
  本物の `PlaybackAudio` の描画中に、時計の `locate` で panic させる。`ForbidAlloc` の内側で、`GuardAlloc` も入っている。
  2 ブロックは音が出て、3 ブロック目以降は無音で、プレイヤーへ再び入らない。
  `service` は場所付きのエラーを返し、記録は 1 行である。
  ガードの変更だけを戻すと、このテストのプロセスは SIGABRT で終わる（2026-11-03 に確認）。
- `panic_log::tests`: 捕捉と 2 回目以降の不実行、panic しないコールバック、長い文の切り詰め、
  ほかのスレッドの追記（スレッド名・バックトレース）、記録の移動。
- `audition::tests::a_panicked_audition_callback_stops_its_sound_with_the_reason`: 試聴の要求の理由。
- `scripts/tests/test_diagnostics.py`: `make debug-report` が panic の記録を写す。

## 残ること

- 音声の経路のモジュールで `clippy::indexing_slicing` を有効にする作業は残る。
  2026-11-03 に数えると、`echode-engine` だけで約 140 か所（`plugins.rs` 80、`mix.rs` 20 など）ある。
  この捕捉は、その間の安全網であり、添字の panic をなくすものではない。
- panic したブロックでは、文の整形と payload の箱の確保がオーディオスレッドで起きる。std の panic の仕組みによる。
- ヘルパー（別プロセス）の中の panic は、親にはヘルパーの終了として見える。理由を親の記録へ運ぶ仕組みはない。
- release のアプリで実際に panic させた確認はしていない。記録の場所の実ファイルはユーザーの環境で初めて作られる。
