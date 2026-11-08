# ADR 0284: 音声経路の crate で添字・unwrap・expect・panic を拒否する

- 日付: 2026-11-08
- 状態: 採用
- 対象: [統合表](../reviews/2026-11-08-open-items-register.md) の R4（[XCA-14](../reviews/2026-11-02-cross-cutting-audit.md#xca-14-コールバックの-panic) の残り）。
- 関連: [ADR 0238](0238-audio-callback-panics.md)（コールバックの panic の捕捉）、[ADR 0276](0276-priority-order-and-delegated-decisions.md)、[ADR 0278](0278-audit-backlog-order.md)、[10/8 監査](../reviews/2026-10-08-audit.md) の ENG-1 の lint の方針。

## 問題

開始時の HEAD は `88429a1`。

- ADR 0238 はコールバックの panic を捕まえて無音にした。捕捉は安全網で、panic したブロック以降の再生は止まる。
  音声の経路で `clippy::indexing_slicing` を有効にする作業が残っていた。
- 拒否はモジュール単位で、engine の `playback.rs`・`audition.rs` と、audio-io の `stream.rs`・`io_cycle.rs`・`audition.rs`・`opening.rs`・`panic_log.rs` だけだった。
  統合表の「audio-io の 4 ファイル」は、実際は 5 ファイルだった。
- 同じ 4 つの lint（`indexing_slicing`・`unwrap_used`・`expect_used`・`panic`）を、音声スレッドで動くコードを持つ crate の lib に掛けると、警告は 291 か所だった。
  engine 159（`plugins.rs` 88、`mix.rs` 21、spike 専用の `graph.rs` 10 など）、realtime 55、sandbox 35、plugin-host 18、time 13、audio-io 6、dsp 3、sine-pad 2。
- 作業中に次が分かった。
  - `echode-realtime/clippy.toml` はワークスペースの `clippy.toml` を置き換える。このため realtime では `Arc::as_ptr`・`Rc::as_ptr` の禁止（HIS-1）が効いていなかった。
  - カーソルの `place` は、生きている音を 1 つずつ `clear` し、数が 0 になるまで繰り返していた。逆引きが壊れていれば `clear` は何もせず、繰返しが終わらない。
  - 計画とカーソルが合わない時（終端の索引がノートの外を指す、ノートがカーソルのバッファより多い）、カーソルは添字の範囲外で panic した。

## 比べた案

1. **モジュールごとに付ける（従来）**: 音声スレッドが通るモジュールを選んで付ける。
   新しいモジュールと、制御と音声が混ざるモジュール（`plugins.rs`）の境界を毎回判断する。付け忘れを止める手段がない。
2. **lib のルートに付ける（採用）**: 対象の crate の lib 全体を拒否する。
   制御側のコード（制御ソケットの解析、プラグインの走査）も入る。そこは信頼しないヘルパーのバイトを読む経路や、panic で別スレッドが止まる経路で、書き換えても損がない。
   規則は 1 つで、新しいモジュールは自動で入る。
3. **`Cargo.toml` の `[lints]` と `clippy.toml` の `allow-*-in-tests`**: `tests/` の統合テストと検査用の bin（`#[test]` の外の補助関数）も対象になり、検査用コードの書換えが大きい。採らない。
4. **捕捉だけに頼る**: ADR 0238 の捕捉は残すが、panic を起こす書き方を残す理由にならない。採らない。

## 決定

- **対象**: 次の crate の lib のルートで `#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used, clippy::panic))]` を宣言する。
  モジュール単位の宣言は消した。
  - `echode-time`（テンポの索引）、`echode-realtime`（カーソル）、`echode-dsp`・`echode-sine-pad`（同梱の音源）、
    `echode-sandbox`（共有メモリと制御ソケット）、`echode-plugin-host`（ヘルパー）、`echode-engine`、`echode-audio-io`。
- **対象外**: テスト（`cfg(test)` と `tests/`）と bin（引数の解析、計測の入口、検査用ヘルパー）。
  spike feature 専用の `graph.rs`（S2 の計測の作業量）は、記録した測定と比べられる形を保つため、モジュールで `allow` する。
- **書き方**:
  - 添字は `get`・`get_mut`、反復子の `zip`・`take`・`skip`・`peekable`、スライスのパターン（`let [left, right] = master`）、
    `first_chunk`・`split_first_chunk`・`split_last_chunk`・`as_chunks`・`split_at_mut_checked`・`checked_rem` に置き換える。
  - 届かない値の扱いは、その場の既存の意味に合わせる。音を作る処理はそのサンプル・ノート・記録を飛ばすか、既存のエラー（`BufferTooSmall` など）を返す。
    ヘルパーの段が見つからない時は、そのブロックを取りこぼし（`MISS_SUBMIT`）として既存の減衰で隠す。制御ソケットの解析は既存の `InvalidData` を返す。
  - 不変条件で必ず届く値に既定値を足す時は、その不変条件をコメントに書く。
- **構造**:
  - `plugins.rs` は、トラックごとの処理（入力の準備、イベントの振り分け、段の投入、進行、非有限値の封じ込め、素材の注入）を `TrackHost` のメソッドに移した。
    ブロックの報告（イベントの取りこぼし、書出しの理由）は `BlockReport` に分けた。コールバックはトラックを 1 回引き、以後は添字を持ち回らない。
  - realtime の `place` は、生きている音の逆引きを消して数を 0 にする。`EndIndex::new` は木を下から埋める。
  - `echode-realtime/clippy.toml` にワークスペースの禁止（HIS-1）を足した。
- **検査**: `scripts/check_audio_lints.py`（`make check` の一部）が、対象の crate のルートの宣言の欠落と、局所の `allow`・`expect` による解除を拒否する。
  解除を許すのは `graph.rs` だけである。対象の crate の一覧はこのスクリプトが正本で、音声スレッドで動く crate を足したら一覧に足す。

## 理由

- ADR 0276 の堅牢性: panic は音切れで、捕捉しても以後のブロックは無音になる。
- 案 2 は規則が 1 つで、付け忘れを検査で止められる。モジュールの境界の判断が要らない。
- `plugins.rs` をメソッドに分けたのは、添字を `get` に置き換えるだけでは、同じトラックを 1 つの関数で何度も引き直す形が残るからである。
  トラックを 1 回引いて渡すと、引き直しも、見つからない時の分岐も 1 か所になる。

## 検証

- `cursor_tests::a_plan_that_does_not_match_its_cursor_plays_what_it_can`: 終端の索引がノートの外を指す計画と、カーソルのバッファより多いノートの計画で、panic せずに鳴らせる音を出す。
  HEAD では `index out of bounds: the len is 4 but the index is 99` で失敗した（2026-11-08）。
- `control::tagged_tests::every_truncated_body_is_refused_without_a_panic`: 構造のある制御メッセージを全ての位置で切って解析する。
  長さの違う配置と型のない本文は `InvalidData` になる。HEAD でも合格した（既存の長さの確認が守っていた）。書換えの回帰として置く。
- `scripts/tests/test_audio_lints.py`: 宣言の欠落・lint の欠け・局所の解除を拒否し、`expect("...")` の呼出しを解除と取り違えない。
- 既存の回帰: engine の `block_size`（実ヘルパーでの期限・取りこぼし）と製品契約。
  `stereo-plugin-chain`・`track-event-routing` の反例は書き換えたコードを指すように置き換え、同じ壊し方（右の平面を捨てる、全イベントを先頭のトラックへ送る）を保った。
- 性能: `spike-plugin-tracks`（release、機器なし）を、HEAD の作業ツリーと変更後で続けて測った（2026-11-09、負荷平均 約 2）。
  ブロックの CPU の p50 は、ヘルパーありの 256 トラックで 1,040.9 → 1,050.3 µs、ヘルパーなしの和音の 256 トラックで 142.7 → 140.2 µs、
  ループの折り返しで 1,297.3 → 1,281.6 µs、計画の採用で 1,417.8 → 1,249.2 µs だった。取りこぼしはどちらも 0 で、差は測定のばらつきの範囲である。

## 残ること

- lint は、整数の 0 除算と、`split_at_mut`・`swap`・`copy_from_slice` の長さの不一致による panic を止めない。
  今回触れた箇所は確かめた書き方にしたが、全体は範囲外である。
- bin とテストは対象外である。
- R5 以降は統合表の順に進める。
