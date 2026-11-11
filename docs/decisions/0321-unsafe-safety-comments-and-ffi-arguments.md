# ADR 0321: `unsafe` の全ブロックに根拠を書き、C ABI のポインター引数を一つの規則で検査する

日付: 2026-11-12
状態: 採用
関連: M2（XCA-15・FFI-A1）、[ADR 0278](0278-audit-backlog-order.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)、[ADR 0284](0284-audio-path-panic-lints.md)

## 問題と再現

開始時の HEAD は `03b3081`。統合表の M2 は「633 ブロックに `SAFETY` 233。`echode-ffi` は 235 に 0。lint は無効」だった。

`clippy::undocumented_unsafe_blocks` と `clippy::unnecessary_safety_comment` を仮に有効にして数えた。
`cargo clippy --workspace --all-targets` で 536 件（根拠のないブロック 491、`unsafe impl` 40、`unsafe` のない所の `SAFETY` 5）。
非既定の feature を加えると、エンジンの spike で 19、`spikes/bridge` の全 feature で 5 件が増える。
`echode-ffi` は 239 件（製品 45、テスト 194）である。

FFI-A1（長さの検査が関数ごとに不揃い）は、次の形で残っていた。

- `echode_document_submit` は `isize::MAX` を超える長さを確かめず、null と 0 でない長さの組を空の JSON として扱った。
  同じ形の `echode_document_audio`・`history`・`device` は、どちらも拒否していた。
- `echode_snapshot_visible`・`echode_snapshot_piano_visible` は、要素の大きさを掛けたバイト数を確かめずにスライスを作った。
  `echode_snapshot_waveform` も同じで、さらに呼出し側の容量だけの作業領域を `vec!` で確保した（確保できなければ中断）。
- 出力の数・長さのポインターは、位置のずれを確かめずに書いた。
- `document_ref`・`snapshot_ref`・`out_bytes`・`c_str` は、生ポインターを参照外しする安全な関数（`unsafe fn` でない）だった。
- 可視列とピアノの出力は、Swift が `.allocate(capacity:)` で確保した初期化前の領域を `&mut [VisibleElement]` として借りていた。

公開の入口の回帰を先に足し、修正前の HEAD で動かした。

- 位置のずれた `length` を渡すと `misaligned pointer dereference` で異常終了した（デバッグの検査、`extern "C"` の panic は中断）。
- `isize::MAX` を超える容量は `unsafe precondition(s) violated: slice::from_raw_parts_mut` で異常終了した。
- `submit` に null と長さ 4 を渡すと `ECHODE_OK` が返り、続く大きすぎる長さでテストのプロセスが SIGSEGV で落ちた。

## 比べた案と決定

### C ABI の引数

1. 注釈だけを足し、各関数の検査は今のままにする。不揃い（FFI-A1）が残るので採らない。
2. 45 の入口それぞれで検査を揃えて書き直す。同じ規則を 45 か所に別々に書くことになり（AGENTS.md）、採らない。
3. **呼出し側のメモリへの変換を `crates/echode-ffi/src/abi.rs` に集め、全入口がそれを通る。**

3 を採る。監査の修正方針「ポインターの変換を少数の補助へ集めてブロックを減らす」と同じである。

- `input`・`output`・`handle`・`place`・`put`・`uuid`・`text` の 7 つの `unsafe fn` だけが呼出し側のメモリを読み書きする。
- 拒否するのは、0 でない長さ・容量を伴う null、`isize::MAX` を超えるバイト数、型に対して位置のずれたアドレスである。
  どれも、引数が欠けた時と同じ値を返す。null の文書・スナップショットへの答えは、各関数の従来どおりである。
- UUID は位置を問わずに読む（`read_unaligned`）。文字列は NUL 終端を読み、不正な UTF-8 は欠けた引数と同じに扱う。
- 出力の領域は `&mut [MaybeUninit<T>]` として借り、書くだけにする。数・長さは `Place` が生ポインターへ書き、参照を作らない。
- 各入口の `# Safety` は、検査で分からないこと（生きている、長さが合う、ほかの書き手と共有しない）だけを書く。
  共通の規則は crate の文書に一度だけ書く。

挙動の変化は次のとおりで、どれも Swift の呼出しが渡さない引数である（呼出し側を確かめた）。

- `submit` は拒否を `command is null or too long` で即座に完了させ、`ECHODE_ERR` を返す。null の文書でも `ECHODE_ERR` を返す（従来は 0）。
- `query` も null の文書で `ECHODE_ERR` を返す（従来は 0）。
- 目録・概要・プラグインの活動は、null で容量のある `out` を -1 で拒否する（従来は「足りない」の 1）。
- 波形は、確保できない容量に 0 を返す（従来は中断）。

### `unsafe` の根拠

- **ワークスペースの lint で `undocumented_unsafe_blocks` を `forbid`、`unnecessary_safety_comment` を警告にする。**
  前者は `unsafe` のブロックと `impl` に `// SAFETY:` を求め、後者は `unsafe` のなくなった所に残った古い根拠を止める。
  `missing_safety_doc` は `clippy::all` に既にある。`multiple_unsafe_ops_per_block` は、ブロックと注釈を増やすだけで情報が増えないので採らない。
- テストのコードも除外しない。`cfg_attr(test, allow(...))` で除くと、規則に例外が増える。
  `echode-ffi` のテストは Miri で所有権の契約を確かめるもので、根拠がその契約である。
- 局所の解除の防ぎ方は 2 案を比べた。
  1. `check_audio_lints.py` と同じく、ソースの `allow`・`expect` を正規表現で探すスクリプト。属性の書き方の揺れを追い続けることになる。
  2. **`forbid` にする。** rustc が `allow(clippy::undocumented_unsafe_blocks) incompatible with previous forbid`（E0453）で拒否する。
     外部のマクロの展開は lint の対象外で、全 target・非既定の feature で衝突はなかった。
  2 を採る。残る抜け道は、メンバーが `[lints] workspace = true` を書かずにワークスペースの lint ごと外れることで、
  `scripts/check_workspace_lints.py` が全メンバーの継承と、`forbid`・`unsafe_op_in_unsafe_fn = "deny"` の水準を確かめる。
  メンバーの一覧は `check_typed_errors.py` の読み方（ADR 0320）を使い、新しい crate は最初のコミットから対象になる。
- ADR 0278 のとおり crate の境界ごとに注釈を足してコミットし、全件を終えたコミットで lint と検査を有効にした。

### 根拠を書いて分かった所

根拠を書けない所は、注釈ではなくコードを直した。

- **プラグインの拡張の表**: `echode-plugin-host` は、`get_extension` の呼出しと結果の型の付け替えを 8 か所で別々に書いていた。
  `extension.rs` の `table::<T>(plugin, id)` 一つにまとめ、「`T` が `id` の表の型である」ことだけを呼出し側の根拠にした。
- **致命的なシグナルの通知（`echode-sandbox`）**: `arm_abort_notice(&MappedRegion)` は安全な関数なのに、シグナルハンドラーが書き込む生ポインターを保存し、
  その領域を生かしておく仕組みがなかった。単体テストのプロセスでは、領域を捨てた後もハンドラーが残る。
  `&Arc<MappedRegion>` を受け取って参照を一つ持ち続け、前の領域も解放しない（ハンドラーはどのスレッドでもいつでも動き得るため）。
  ヘルパーは 1 プロセスに 1 回しか呼ばないので、残す量は領域 1 つである。使われていなかった `point_abort_notice` は消した。
  回帰は `region_tests.rs` の `the_abort_notice_keeps_its_mapping_for_the_rest_of_the_process` である。
- **HAL の出力の一覧（`echode-audio-io`）**: 製品の出力と試聴の IOProc が、可変長の `AudioBufferList` をスライスにする 3 つの操作を別々に書いていた。
  `stream::hal_buffers` にまとめた。描画の連鎖の `unsafe fn`（`block`・`fill_with_time`・`fill_inner`・`submitted_pair`）に `# Safety` がなかったので書いた。
- **任意の型の `assume_init`（`echode-audio-io`・`echode-history`）**: `hardware::get::<T: Copy>` と `media::audio_property::<T>` は、
  OS が書いたバイトをどの型でも `assume_init` していた。`bool` などの不正なビットの型で呼べば未定義動作になる。
  どのバイトも正しい値である型だけに付ける `unsafe trait PlainProperty` で縛り、根拠を実装の側へ移した
  （`u32`・`f64`・`i64`・`AudioStreamBasicDescription`・生ポインター）。呼出しは変えていない。
- **エンジンの spike**: `graph::workgroup_parallel_threads` と `device_measurement::silence` は、生ポインターを受けて参照外しする安全な関数だった。
  `unsafe fn` にし、呼出し側に根拠を書いた。計測の処理は変えていない（ADR 0284 の除外と同じく、計測したコードの動きを保つ）。
- **意図した未定義動作**: テスト用プラグインの `segfault` の故障と、橋渡しの spike のサニタイザの対照は、未定義動作そのものが目的である。
  根拠の欄には「意図した故障で、期待どおり失敗する子プロセスだけが実行する」と書いた。

## 結果

- lint を仮に有効にした時の 560 か所（全 target 536、エンジンの spike 19、橋渡しの全 feature 5）を 0 にした。
  `rtsan`・`headless-output`・`libfuzzer` の feature でも一度 clippy を通した。これらの下に `unsafe` はなく、`make check` はこの組合せを見ない。
- `unsafe` のブロックは 819 → 841、`SAFETY` の注釈は 329 → 902（`crates/` と `spikes/bridge` の grep、テストを含む）。
  ブロックが増えたのは、`echode-ffi` の参照外しを隠していた安全な補助関数をなくし、各入口が `abi` を呼ぶ所を明示したためである。
  呼出し側のメモリを実際に読み書きする所は `abi.rs` の 7 つのブロックだけになった。
- 音声の処理・ブロック長・保存形式・Swift の呼出しは変えていない。性能の改善は主張しない。

## 回帰と基盤

- `abi_tests.rs`（Miri でも実行）は、`isize::MAX` を超える出力の容量、0 でない長さの null、位置のずれた出力、位置を問わない UUID を、
  公開の入口で照合する。修正前は 4 件中 2 件が異常終了した（上の再現）。残る 2 件は従来からの答えを固定する。
- `tests.rs` の `contract_queued_commands_refuse_null_and_oversized_bytes_at_once` は、4 つの入口の拒否、完了の文、返り値と、拒否の後も文書が使えることを照合する。
  修正前は SIGSEGV で失敗した。
- `scripts/check-bridge-miri.sh` は `abi_tests` も各 seed で動かす。`check-bridge-miri` の `task_inputs` に `crates/echode-ffi/src/abi*` を足した。
  選択の回帰は `test_verification_tiers.py` の `test_the_c_abi_argument_rules_reach_miri_at_the_task_tier`。
- `check_workspace_lints.py` を `check-base` と `check-portable` で動かし、`check` の入力に足した。回帰は `scripts/tests/test_workspace_lints.py`。
- コードの所在（`docs/development.md`）、検査手順（`docs/verification.md`）、診断（`docs/debugging/verification-and-build.md`）、
  監査の状態（統合表の M2、XCA-15、FFI-A1）を更新した。利用者の制約と実機の確認場面は増えず、status.md とユーザー確認の一覧は変えない。

## M2 の完了

統合表の M2（XCA-15・FFI-A1）を対応済みとした。ADR 0278 の順で、次は M4（ログの仕組み）である。
